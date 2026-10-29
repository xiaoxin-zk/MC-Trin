use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::SystemTime;

use chrono::{DateTime, Utc};

use crate::State;
use crate::state::instances::DependencyRelation;
use crate::state::instances::{
    ContentItemCapabilities, ContentOwnershipKind, InstanceContentSnapshot,
    InstanceContentSnapshotItem, InstanceLink, LoaderComponentKind,
    PackMemberMaterializationState, PackMemberOverrideKind,
};
use crate::state::{
    ContentItem, ContentProvider, ContentProviderRef, ContentSourceKind,
    DependencyType, InstanceUpgradeAction, InstanceUpgradeDependencyChange,
    InstanceUpgradeDependencyChangeKind, InstanceUpgradeDependencyRequirement,
    InstanceUpgradeEnvironment, InstanceUpgradeFixedConstraint,
    InstanceUpgradeIssue, InstanceUpgradeIssueCode, InstanceUpgradeItem,
    InstanceUpgradeItemStatus, InstanceUpgradePlan, InstanceUpgradeResolution,
    InstanceUpgradeSelection, InstanceUpgradeSolution,
    InstanceUpgradeSolutionKind, InstanceUpgradeSourceFile, ModrinthProjectId,
    ProjectType, ShaderRuntime, Version,
};

mod solver;
use solver::{
    compare_minimal, compare_newest, select_upgrade_failure_issue,
    solve_upgrade_with_fixed_roots,
};
#[cfg(test)]
use solver::{
    root_candidate_options, solve_for_strategy, solve_for_strategy_with_limit,
    solve_upgrade,
};

const MAX_CANDIDATES_PER_PROJECT: usize = 6;
// TODO(stage 3): Custom UI candidate listing needs an independent backend API.
// This solver bound is not, and must not become, the UI version-list limit.
const MAX_SEARCH_STATES: usize = 10_000;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct NodeKey {
    provider: ContentProvider,
    project_id: String,
}

impl NodeKey {
    fn new(provider: ContentProvider, project_id: impl Into<String>) -> Self {
        Self {
            provider,
            project_id: project_id.into(),
        }
    }

    fn label(&self) -> String {
        format!("{}:{}", self.provider.as_str(), self.project_id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CandidateChannel {
    Release,
    Beta,
    Alpha,
}

impl CandidateChannel {
    fn rank(self) -> u8 {
        match self {
            Self::Release => 3,
            Self::Beta => 2,
            Self::Alpha => 1,
        }
    }

    fn is_prerelease(self) -> bool {
        self != Self::Release
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CandidateDependencyKind {
    Required,
    Optional,
    Incompatible,
    Embedded,
}

#[derive(Clone, Debug)]
struct CandidateDependency {
    key: NodeKey,
    version_id: Option<String>,
    kind: CandidateDependencyKind,
}

#[derive(Clone, Debug)]
struct UpgradeCandidate {
    key: NodeKey,
    version_id: String,
    published: DateTime<Utc>,
    channel: CandidateChannel,
    compatible: bool,
    installed_current: bool,
    dependencies: Vec<CandidateDependency>,
}

#[derive(Clone, Debug, Default)]
struct CandidatePool {
    candidates: Vec<UpgradeCandidate>,
    exploration_limited: bool,
    has_target_game_version_release: bool,
}

type UpgradeCatalog = HashMap<NodeKey, CandidatePool>;

#[derive(Clone, Debug)]
struct InstalledAlias {
    key: NodeKey,
    current_release_id: String,
}

#[derive(Clone, Debug)]
struct InstalledNode {
    content_id: String,
    key: NodeKey,
    current_release_id: String,
    project_type: ProjectType,
    enabled: bool,
    auto_dependency: bool,
    user_owned: bool,
    migratable: bool,
    aliases: Vec<InstalledAlias>,
}

#[derive(Clone, Debug)]
struct RootRequest {
    content_id: String,
    key: NodeKey,
    current_release_id: String,
    enabled: bool,
    action: InstanceUpgradeAction,
    allow_prerelease: bool,
}

#[derive(Clone, Debug)]
struct Requirement {
    key: NodeKey,
    version_id: Option<String>,
    explicit_prerelease: bool,
    preserve_unsafe: bool,
    root_content_id: String,
    root_key: NodeKey,
    origins: Vec<InstanceUpgradeDependencyRequirement>,
}

#[derive(Clone, Debug)]
struct SolverResult {
    assignments: HashMap<NodeKey, UpgradeCandidate>,
    physical_assignments: HashMap<String, NodeKey>,
    preserved_unsafe: HashSet<NodeKey>,
}

#[derive(Default)]
struct SearchState {
    visited: usize,
    limit_reached: bool,
    first_issue: Option<InstanceUpgradeIssue>,
}

#[derive(Clone)]
pub(crate) struct ReadOnlyUpgradeSource {
    snapshot: InstanceContentSnapshot,
    source_files: Vec<InstanceUpgradeSourceFile>,
    instance_path: String,
    game_dir: std::path::PathBuf,
    file_states: Vec<UpgradeSourceFileState>,
    catalog: Option<UpgradeCatalog>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UpgradeSourceFileState {
    relative_path: String,
    size: u64,
    enabled: bool,
    modified: Option<SystemTime>,
}

pub(crate) struct UpgradePlanRuntimeValidation {
    source: ReadOnlyUpgradeSource,
    watcher_epoch: Option<u64>,
    validated_generation: Option<u64>,
    #[cfg(test)]
    full_hash_validations: usize,
    #[cfg(test)]
    incremental_hashes: usize,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum PhysicalNodeIdentity {
    Installed(String),
    Project(NodeKey),
}

#[derive(Default)]
struct InstalledAliasIndex {
    content_id_by_alias: HashMap<NodeKey, String>,
    aliases_by_content_id: HashMap<String, HashMap<NodeKey, String>>,
}

impl InstalledAliasIndex {
    fn new(installed: &[InstalledNode]) -> Self {
        let mut index = Self::default();
        for node in installed {
            for alias in &node.aliases {
                index
                    .content_id_by_alias
                    .insert(alias.key.clone(), node.content_id.clone());
                index
                    .aliases_by_content_id
                    .entry(node.content_id.clone())
                    .or_default()
                    .insert(
                        alias.key.clone(),
                        alias.current_release_id.clone(),
                    );
            }
        }
        index
    }

    fn content_id(&self, key: &NodeKey) -> Option<&str> {
        self.content_id_by_alias.get(key).map(String::as_str)
    }

    fn same_physical_content(&self, left: &NodeKey, right: &NodeKey) -> bool {
        self.content_id(left).is_some()
            && self.content_id(left) == self.content_id(right)
    }

    fn current_release(&self, content_id: &str, key: &NodeKey) -> Option<&str> {
        self.aliases_by_content_id
            .get(content_id)
            .and_then(|aliases| aliases.get(key))
            .map(String::as_str)
    }

    fn physical_identity(&self, key: &NodeKey) -> PhysicalNodeIdentity {
        self.content_id(key).map_or_else(
            || PhysicalNodeIdentity::Project(key.clone()),
            |content_id| {
                PhysicalNodeIdentity::Installed(content_id.to_string())
            },
        )
    }
}

pub(crate) async fn create_instance_upgrade_plan_with_source(
    instance_id: &str,
    target_environment: InstanceUpgradeEnvironment,
    state: &State,
) -> crate::Result<(InstanceUpgradePlan, ReadOnlyUpgradeSource)> {
    let metadata = super::get_instance_metadata(instance_id, &state.pool)
        .await?
        .ok_or_else(|| {
            crate::ErrorKind::InputError("Unknown instance".to_string())
        })?;
    if !matches!(metadata.link, InstanceLink::Unmanaged) {
        return Err(crate::ErrorKind::InputError(
            "Instance upgrade planner only supports unmanaged instances"
                .to_string(),
        )
        .into());
    }
    if metadata.applied_content_set.source_kind != ContentSourceKind::Local {
        return Err(crate::ErrorKind::InputError(
            "Instance upgrade planner only supports local content sets"
                .to_string(),
        )
        .into());
    }

    let mut source = read_only_upgrade_source(instance_id, state).await?;
    let snapshot = source.snapshot.clone();
    if snapshot.revision != metadata.applied_content_set.revision {
        return Err(crate::ErrorKind::InputError(
            "Instance content changed while the upgrade plan was being created; retry planning"
                .to_string(),
        )
        .into());
    }
    let source_environment = InstanceUpgradeEnvironment {
        game_version: metadata.applied_content_set.game_version.clone(),
        mod_loader: metadata.applied_content_set.loader,
        mod_loader_version: metadata.applied_content_set.loader_version.clone(),
        shader_runtime: source_shader_runtime(
            &metadata.loader_components,
            &snapshot,
        ),
    };
    let (mut items, installed) = snapshot_upgrade_items(&snapshot);
    let root_types = installed
        .iter()
        .filter(|node| !node.auto_dependency && node.migratable)
        .map(|node| (node.key.clone(), node.project_type))
        .collect::<HashMap<_, _>>();
    let fixed = FixedRootConstraints::default();
    let catalog = load_upgrade_catalog(
        &root_types,
        &installed,
        &fixed,
        &target_environment,
        state,
    )
    .await?;
    classify_items(&mut items, &installed, &catalog, &target_environment);

    let roots = roots_from_items(&items, &installed);
    let outcome = solve_upgrade_with_fixed_roots(
        &roots,
        &installed,
        &catalog,
        &fixed,
        &confirmed_prereleases(&items),
    );
    apply_solver_issues_to_items(&mut items, &outcome.issues);
    let mut blocking_issues = outcome.issues;
    for item in &items {
        if let Some(issue) = blocking_issue_for_item(item, false) {
            blocking_issues.push(issue);
        }
    }
    deduplicate_issues(&mut blocking_issues);
    let warnings = item_warnings(&items);
    let newest_solution = outcome
        .solutions
        .iter()
        .max_by(|left, right| compare_newest(left, right, &roots))
        .map(|solution| {
            materialize_solution(
                InstanceUpgradeSolutionKind::Newest,
                solution,
                &roots,
                &installed,
            )
        });
    let minimal_change_solution = outcome
        .solutions
        .iter()
        .min_by(|left, right| compare_minimal(left, right, &roots, &installed))
        .map(|solution| {
            materialize_solution(
                InstanceUpgradeSolutionKind::MinimalChange,
                solution,
                &roots,
                &installed,
            )
        });
    let selected_solution = newest_solution.clone();
    let dependency_changes = selected_solution
        .as_ref()
        .map(|solution| solution.dependency_changes.clone())
        .unwrap_or_default();

    let plan = InstanceUpgradePlan {
        id: format!("instance-upgrade-plan:{}", uuid::Uuid::new_v4()),
        instance_id: instance_id.to_string(),
        source_revision: snapshot.revision,
        source_files: source.source_files.clone(),
        source_environment,
        target_environment,
        items,
        dependency_changes,
        warnings,
        blocking_issues,
        newest_solution,
        minimal_change_solution,
        selected_solution,
        custom_constraints: Vec::new(),
    };
    source.catalog = Some(catalog);
    Ok((plan, source))
}

async fn read_only_upgrade_source(
    instance_id: &str,
    state: &State,
) -> crate::Result<ReadOnlyUpgradeSource> {
    let mut snapshot =
        super::get_cached_content_snapshot(instance_id, state).await?;
    let instance = crate::state::instances::adapters::sqlite::instance_rows::get_instance_by_id(
        instance_id,
        &state.pool,
    )
    .await?
    .ok_or_else(|| crate::ErrorKind::InputError("Unknown instance".to_string()))?;
    let instance_dir = upgrade_source_game_dir(&instance, &state.directories)?;
    let scanned =
        crate::state::instances::adapters::filesystem::scan_content_files_from(
            &instance_dir,
            &instance.path,
        )?;
    let file_states = scanned
        .iter()
        .map(|file| UpgradeSourceFileState {
            relative_path: file.relative_path.clone(),
            size: file.size,
            enabled: file.enabled,
            modified: file.modified,
        })
        .collect();
    let mut scanned_by_path = HashMap::new();
    let mut source_files = Vec::new();
    for file in scanned {
        let (_, sha1) = crate::util::fetch::sha1_file_async(
            instance_dir.join(&file.relative_path),
        )
        .await?;
        source_files.push(InstanceUpgradeSourceFile {
            relative_path: file.relative_path.clone(),
            sha1: sha1.clone(),
            size: file.size,
            enabled: file.enabled,
        });
        scanned_by_path.insert(file.relative_path.clone(), (file, sha1));
    }
    snapshot.items.retain(|item| {
        if !crate::state::instances::adapters::filesystem::is_scannable_project_path(
            item.project_type,
            &item.expected_relative_path,
        ) {
            return true;
        }
        scanned_by_path
            .get(&item.expected_relative_path.replace('\\', "/"))
            .is_some_and(|(_, sha1)| {
                item.content.as_ref().is_some_and(|content| {
                    content.id.eq_ignore_ascii_case(sha1)
                })
            })
    });
    let represented = snapshot
        .items
        .iter()
        .map(|item| item.expected_relative_path.replace('\\', "/"))
        .collect::<HashSet<_>>();
    for (_, (file, sha1)) in scanned_by_path {
        if represented.contains(&file.relative_path) {
            continue;
        }
        let Some(project_type) = crate::state::instances::adapters::filesystem::project_type_from_relative_path(
            &file.relative_path,
        ) else {
            continue;
        };
        snapshot.items.push(InstanceContentSnapshotItem {
            artifact_ref: None,
            compatibility: None,
            resource_id: None,
            pack_resource_id: None,
            file_id: None,
            entry_id: None,
            member_id: None,
            ownership_kind: ContentOwnershipKind::LocalDiscovered,
            materialization_state: PackMemberMaterializationState::Present,
            override_kind: PackMemberOverrideKind::None,
            expected_relative_path: file.relative_path.clone(),
            required: false,
            project_type,
            provider: None,
            provider_project_id: None,
            provider_release_id: None,
            content: Some(ContentItem {
                file_name: file.file_name,
                file_path: file.relative_path,
                id: sha1,
                size: file.size,
                enabled: file.enabled,
                project_type,
                project: None,
                version: None,
                owner: None,
                update: None,
                date_added: None,
                provider_refs: Vec::new(),
                origin_provider: None,
                rollback: None,
                environment: None,
                source_kind: None,
                external: true,
                loader: None,
            }),
            capabilities: ContentItemCapabilities::default(),
            dependency: None,
        });
    }
    source_files
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(ReadOnlyUpgradeSource {
        snapshot,
        source_files,
        instance_path: instance.path,
        game_dir: instance_dir,
        file_states,
        catalog: None,
    })
}

fn upgrade_source_game_dir(
    instance: &crate::state::Instance,
    directories: &crate::state::DirectoryInfo,
) -> crate::Result<std::path::PathBuf> {
    Ok(
        crate::launcher::instance_runtime::InstanceRuntimeAdapter::for_instance(
            instance,
            directories,
        )?
        .game_dir(),
    )
}

pub(crate) async fn validate_instance_upgrade_plan_source(
    plan: &InstanceUpgradePlan,
    state: &State,
) -> crate::Result<ReadOnlyUpgradeSource> {
    let current = read_only_upgrade_source(&plan.instance_id, state).await?;
    ensure_upgrade_source_files_match(
        &plan.instance_id,
        &plan.source_files,
        &current.source_files,
    )?;
    Ok(current)
}

pub(crate) async fn scan_instance_upgrade_source_files(
    instance_id: &str,
    state: &State,
) -> crate::Result<Vec<InstanceUpgradeSourceFile>> {
    Ok(read_only_upgrade_source(instance_id, state)
        .await?
        .source_files)
}

impl UpgradePlanRuntimeValidation {
    pub(crate) async fn new(
        source: ReadOnlyUpgradeSource,
        instance_id: &str,
        creation_watch: Option<
            crate::state::instances::watcher::InstanceContentWatchSnapshot,
        >,
        state: &State,
    ) -> Self {
        let watch = state
            .file_watcher
            .track_upgrade_source(
                instance_id,
                source
                    .source_files
                    .iter()
                    .map(|file| file.relative_path.clone()),
            )
            .await;
        let validated_generation = creation_watch
            .as_ref()
            .filter(|creation| {
                watch
                    .as_ref()
                    .is_some_and(|current| current.epoch == creation.epoch)
            })
            .map(|creation| creation.generation);
        Self {
            source,
            watcher_epoch: watch.as_ref().map(|watch| watch.epoch),
            validated_generation,
            #[cfg(test)]
            full_hash_validations: 0,
            #[cfg(test)]
            incremental_hashes: 0,
        }
    }

    pub(crate) async fn validate(
        &mut self,
        plan: &InstanceUpgradePlan,
        state: &State,
    ) -> crate::Result<ReadOnlyUpgradeSource> {
        let Some(mut before) = state
            .file_watcher
            .content_watch_snapshot(&plan.instance_id)
            .await
        else {
            // Unwatched game directories still support metadata and hash
            // validation through the resolved source directory.
            self.incremental_validate(plan, None, state).await?;
            return Ok(self.source.clone());
        };
        if self.watcher_epoch != Some(before.epoch)
            || self.validated_generation.is_none()
            || self.validated_generation > Some(before.generation)
        {
            return self.authoritative_validate(plan, state).await;
        }

        for _ in 0..2 {
            self.incremental_validate(plan, Some(&before), state)
                .await?;
            let Some(after) = state
                .file_watcher
                .content_watch_snapshot(&plan.instance_id)
                .await
            else {
                return self.authoritative_validate(plan, state).await;
            };
            if after.epoch == before.epoch
                && after.generation == before.generation
            {
                self.watcher_epoch = Some(after.epoch);
                self.validated_generation = Some(after.generation);
                return Ok(self.source.clone());
            }
            before = after;
        }

        self.authoritative_validate(plan, state).await
    }

    async fn incremental_validate(
        &mut self,
        plan: &InstanceUpgradePlan,
        watch: Option<
            &crate::state::instances::watcher::InstanceContentWatchSnapshot,
        >,
        _state: &State,
    ) -> crate::Result<()> {
        let scanned = crate::state::instances::adapters::filesystem::scan_content_files_from(
            &self.source.game_dir,
            &self.source.instance_path,
        )?;
        let planned = self
            .source
            .source_files
            .iter()
            .map(|file| (file.relative_path.as_str(), file))
            .collect::<HashMap<_, _>>();
        let previous = self
            .source
            .file_states
            .iter()
            .map(|file| (file.relative_path.as_str(), file))
            .collect::<HashMap<_, _>>();
        if scanned.len() != planned.len()
            || scanned
                .iter()
                .any(|file| !planned.contains_key(file.relative_path.as_str()))
        {
            return stale_upgrade_source(&plan.instance_id);
        }

        let watcher_changed = watch.is_some_and(|watch| {
            self.validated_generation != Some(watch.generation)
        });
        let instance_dir = &self.source.game_dir;
        for file in &scanned {
            let planned_file = planned[&file.relative_path.as_str()];
            if file.size != planned_file.size
                || file.enabled != planned_file.enabled
            {
                return stale_upgrade_source(&plan.instance_id);
            }
            let metadata_changed = previous
                .get(file.relative_path.as_str())
                .is_none_or(|previous| previous.modified != file.modified);
            let watcher_marked = watcher_changed
                && watch.is_some_and(|watch| {
                    watch.dirty_paths.contains(&file.relative_path)
                });
            if metadata_changed || watcher_marked {
                let (_, sha1) = crate::util::fetch::sha1_file_async(
                    instance_dir.join(&file.relative_path),
                )
                .await?;
                #[cfg(test)]
                {
                    self.incremental_hashes += 1;
                }
                if !planned_file.sha1.eq_ignore_ascii_case(&sha1) {
                    return stale_upgrade_source(&plan.instance_id);
                }
            }
        }
        self.source.file_states = scanned
            .into_iter()
            .map(|file| UpgradeSourceFileState {
                relative_path: file.relative_path,
                size: file.size,
                enabled: file.enabled,
                modified: file.modified,
            })
            .collect();
        Ok(())
    }

    async fn authoritative_validate(
        &mut self,
        plan: &InstanceUpgradePlan,
        state: &State,
    ) -> crate::Result<ReadOnlyUpgradeSource> {
        #[cfg(test)]
        {
            self.full_hash_validations += 1;
        }
        let current =
            validate_instance_upgrade_plan_source(plan, state).await?;
        let watch = state
            .file_watcher
            .track_upgrade_source(
                &plan.instance_id,
                current
                    .source_files
                    .iter()
                    .map(|file| file.relative_path.clone()),
            )
            .await;
        self.watcher_epoch = watch.as_ref().map(|watch| watch.epoch);
        self.validated_generation =
            watch.as_ref().map(|watch| watch.generation);
        let catalog = self.source.catalog.clone();
        self.source = current;
        self.source.catalog = catalog;
        Ok(self.source.clone())
    }
}

fn stale_upgrade_source<T>(instance_id: &str) -> crate::Result<T> {
    Err(crate::ErrorKind::StaleInstanceUpgradePlanSource {
        instance_id: instance_id.to_string(),
    }
    .into())
}

fn ensure_upgrade_source_files_match(
    instance_id: &str,
    planned: &[InstanceUpgradeSourceFile],
    current: &[InstanceUpgradeSourceFile],
) -> crate::Result<()> {
    if current != planned {
        return Err(crate::ErrorKind::StaleInstanceUpgradePlanSource {
            instance_id: instance_id.to_string(),
        }
        .into());
    }
    Ok(())
}

fn source_shader_runtime(
    components: &[crate::state::LoaderComponent],
    snapshot: &InstanceContentSnapshot,
) -> ShaderRuntime {
    if snapshot.items.iter().any(item_has_iris_identity) {
        return ShaderRuntime::Iris;
    }
    if components
        .iter()
        .any(|component| component.kind == LoaderComponentKind::OptiFine)
    {
        return ShaderRuntime::OptiFine;
    }
    if snapshot.items.iter().any(|item| {
        item.project_type == ProjectType::Mod
            && item
                .content
                .as_ref()
                .is_none_or(|content| content.provider_refs.is_empty())
    }) {
        ShaderRuntime::Unknown
    } else {
        ShaderRuntime::None
    }
}

fn item_has_iris_identity(item: &InstanceContentSnapshotItem) -> bool {
    (item.provider == Some(ContentProvider::Modrinth)
        && item.provider_project_id.as_deref() == Some("YL57xq9U"))
        || item.content.as_ref().is_some_and(|content| {
            content.provider_refs.iter().any(|reference| {
                matches!(reference, ContentProviderRef::Modrinth { project_id, .. } if project_id.as_str() == "YL57xq9U")
            })
        })
}

fn snapshot_upgrade_items(
    snapshot: &InstanceContentSnapshot,
) -> (Vec<InstanceUpgradeItem>, Vec<InstalledNode>) {
    let mut items = Vec::new();
    let mut installed = Vec::new();
    for item in &snapshot.items {
        let content_id = stable_content_id(item);
        let current_enabled =
            item.content.as_ref().is_none_or(|content| content.enabled);
        let auto_dependency = item
            .dependency
            .as_ref()
            .is_some_and(|dependency| dependency.auto_dependency);
        let unsupported = is_world_datapack(&item.expected_relative_path)
            || matches!(
                item.project_type,
                ProjectType::Schematic | ProjectType::WorldSave
            );
        let installed_identity = installed_identity(item);
        let recognized = installed_identity
            .as_ref()
            .map(|(key, _, _)| (key.provider, key.project_id.clone()))
            .or_else(|| item.provider.zip(item.provider_project_id.clone()));
        let planner_provider = installed_identity
            .as_ref()
            .map(|(key, _, _)| key.provider)
            .or(item.provider);
        let planner_project_id = installed_identity
            .as_ref()
            .map(|(key, _, _)| key.project_id.clone())
            .or_else(|| item.provider_project_id.clone());
        let planner_release_id = installed_identity
            .as_ref()
            .map(|(_, release_id, _)| release_id.clone())
            .or_else(|| item.provider_release_id.clone());
        let status = if unsupported {
            InstanceUpgradeItemStatus::UnsupportedContentType
        } else if recognized.is_none() {
            InstanceUpgradeItemStatus::Unidentified
        } else {
            InstanceUpgradeItemStatus::NoCompatibleRelease
        };
        let action = if unsupported || recognized.is_none() {
            InstanceUpgradeAction::Keep
        } else {
            InstanceUpgradeAction::Upgrade
        };
        items.push(InstanceUpgradeItem {
            content_id: content_id.clone(),
            relative_path: item.expected_relative_path.clone(),
            project_type: item.project_type,
            provider: planner_provider,
            project_id: planner_project_id,
            current_release_id: planner_release_id,
            current_enabled,
            auto_dependency,
            status,
            resolution: InstanceUpgradeResolution {
                content_id: content_id.clone(),
                action,
                allow_prerelease: false,
                confirmed_prerelease_dependencies: Vec::new(),
            },
            candidate_release_ids: Vec::new(),
        });
        if let Some((key, current_release_id, aliases)) = installed_identity {
            installed.push(InstalledNode {
                content_id,
                key,
                current_release_id: current_release_id.clone(),
                project_type: item.project_type,
                enabled: current_enabled,
                auto_dependency,
                user_owned: item.ownership_kind
                    == ContentOwnershipKind::UserAdded
                    && !auto_dependency,
                migratable: !unsupported,
                aliases,
            });
        }
    }
    (items, installed)
}

fn installed_identity(
    item: &InstanceContentSnapshotItem,
) -> Option<(NodeKey, String, Vec<InstalledAlias>)> {
    let mut aliases = item
        .content
        .as_ref()
        .into_iter()
        .flat_map(|content| &content.provider_refs)
        .filter_map(|reference| {
            Some(InstalledAlias {
                key: NodeKey::new(
                    reference.provider(),
                    reference.database_project_id(),
                ),
                current_release_id: reference.database_release_id()?,
            })
        })
        .collect::<Vec<_>>();
    if let (Some(provider), Some(project_id), Some(release_id)) = (
        item.provider,
        item.provider_project_id.as_deref(),
        item.provider_release_id.as_deref(),
    ) {
        let primary = InstalledAlias {
            key: NodeKey::new(provider, project_id),
            current_release_id: release_id.to_string(),
        };
        aliases.retain(|alias| alias.key != primary.key);
        aliases.insert(0, primary);
    }
    let mut seen = HashSet::new();
    aliases.retain(|alias| seen.insert(alias.key.clone()));
    let primary_index = item
        .provider
        .zip(item.provider_project_id.as_deref())
        .and_then(|(provider, project_id)| {
            aliases.iter().position(|alias| {
                alias.key.provider == provider
                    && alias.key.project_id == project_id
            })
        })
        .unwrap_or(0);
    let primary = aliases.get(primary_index)?.clone();
    if primary_index != 0 {
        aliases.swap(0, primary_index);
    }
    Some((primary.key, primary.current_release_id, aliases))
}

fn stable_content_id(item: &InstanceContentSnapshotItem) -> String {
    item.entry_id
        .as_deref()
        .or(item.member_id.as_deref())
        .or(item.file_id.as_deref())
        .unwrap_or(&item.expected_relative_path)
        .to_string()
}

fn is_world_datapack(path: &str) -> bool {
    let components = path
        .replace('\\', "/")
        .split('/')
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    components.len() >= 4
        && components[0] == "saves"
        && components[2] == "datapacks"
}

struct SolveOutcome {
    solutions: Vec<SolverResult>,
    issues: Vec<InstanceUpgradeIssue>,
    #[cfg(test)]
    visited_states: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SolveStrategy {
    Newest,
    MinimalChange,
}

#[derive(Clone, Debug)]
struct RootCandidateOptions {
    content_id: String,
    key: NodeKey,
    current_release_id: String,
    fixed: bool,
    exploration_limited: bool,
    candidates: Vec<Option<UpgradeCandidate>>,
}

#[derive(Clone, Debug, Default)]
struct FixedRootConstraints {
    by_content_id: HashMap<String, (NodeKey, String)>,
    versions_by_project: HashMap<NodeKey, HashSet<String>>,
}

impl FixedRootConstraints {
    fn from_constraints(
        constraints: &[InstanceUpgradeFixedConstraint],
    ) -> Self {
        let mut fixed = Self::default();
        for constraint in constraints {
            let key = NodeKey::new(constraint.provider, &constraint.project_id);
            fixed.by_content_id.insert(
                constraint.content_id.clone(),
                (key.clone(), constraint.version_id.clone()),
            );
            fixed
                .versions_by_project
                .entry(key)
                .or_default()
                .insert(constraint.version_id.clone());
        }
        fixed
    }

    #[cfg(test)]
    fn from_project_versions(
        roots: &[RootRequest],
        versions: &HashMap<NodeKey, String>,
    ) -> Self {
        let constraints = roots
            .iter()
            .filter_map(|root| {
                versions.get(&root.key).map(|version_id| {
                    InstanceUpgradeFixedConstraint {
                        content_id: root.content_id.clone(),
                        provider: root.key.provider,
                        project_id: root.key.project_id.clone(),
                        version_id: version_id.clone(),
                    }
                })
            })
            .collect::<Vec<_>>();
        Self::from_constraints(&constraints)
    }

    fn version_for_root<'a>(&'a self, root: &RootRequest) -> Option<&'a str> {
        self.by_content_id
            .get(&root.content_id)
            .filter(|(key, _)| key == &root.key)
            .map(|(_, version_id)| version_id.as_str())
    }

    fn is_fixed_root(&self, root: &RootRequest) -> bool {
        self.version_for_root(root).is_some()
    }

    fn contains_content(&self, content_id: &str) -> bool {
        self.by_content_id.contains_key(content_id)
    }

    fn versions_for_project(&self, key: &NodeKey) -> Option<&HashSet<String>> {
        self.versions_by_project.get(key)
    }

    fn is_empty(&self) -> bool {
        self.by_content_id.is_empty()
    }
}

#[derive(Clone, Debug)]
struct ConflictSet {
    involved_root_content_ids: HashSet<String>,
    involved_parent_projects: HashSet<NodeKey>,
    dependency_project: Option<NodeKey>,
    reason: InstanceUpgradeIssueCode,
    candidate_limit_roots: HashSet<String>,
}

impl ConflictSet {
    fn can_branch_root(&self, root: &RootCandidateOptions) -> bool {
        let relevant_reason = matches!(
            self.reason,
            InstanceUpgradeIssueCode::DependencyConflict
                | InstanceUpgradeIssueCode::IncompatibleDependency
                | InstanceUpgradeIssueCode::MissingRequiredDependency
                | InstanceUpgradeIssueCode::NoCompatibleRelease
        );
        let is_dependency_target_only = self.dependency_project.as_ref()
            == Some(&root.key)
            && !self.involved_root_content_ids.contains(&root.content_id)
            && !self.involved_parent_projects.contains(&root.key);
        relevant_reason
            && !is_dependency_target_only
            && (self.involved_root_content_ids.contains(&root.content_id)
                || self.involved_parent_projects.contains(&root.key))
    }

    fn same_scope(&self, other: &Self) -> bool {
        self.involved_root_content_ids == other.involved_root_content_ids
            && self.involved_parent_projects == other.involved_parent_projects
            && self.dependency_project == other.dependency_project
            && self.reason == other.reason
    }

    fn merge_candidate_limit_evidence(&mut self, other: &Self) {
        if self.same_scope(other) {
            self.candidate_limit_roots
                .extend(other.candidate_limit_roots.iter().cloned());
        }
    }

    fn candidate_search_incomplete(&self) -> bool {
        !self.candidate_limit_roots.is_empty()
    }
}

#[derive(Clone, Debug)]
struct ConflictFailure {
    issue: InstanceUpgradeIssue,
    conflict: ConflictSet,
}

fn retain_best_failure(
    best_failure: &mut Option<ConflictFailure>,
    failure: ConflictFailure,
) {
    let Some(best) = best_failure.as_mut() else {
        *best_failure = Some(failure);
        return;
    };
    if best.conflict.same_scope(&failure.conflict) {
        best.conflict
            .merge_candidate_limit_evidence(&failure.conflict);
        if failure.issue.dependency_requirements.len()
            > best.issue.dependency_requirements.len()
        {
            best.issue = failure.issue;
        }
    } else if best.conflict.candidate_search_incomplete()
        && !failure.conflict.candidate_search_incomplete()
    {
        *best = failure;
    }
}

struct StrategySolveOutcome {
    solution: Option<SolverResult>,
    issue: Option<InstanceUpgradeIssue>,
    #[cfg(test)]
    visited_states: usize,
}

#[derive(Eq, PartialEq)]
struct FrontierState {
    selected: Vec<usize>,
    primary_score: i64,
    freshness_score: i64,
}

impl Ord for FrontierState {
    fn cmp(&self, other: &Self) -> Ordering {
        self.primary_score
            .cmp(&other.primary_score)
            .then_with(|| self.freshness_score.cmp(&other.freshness_score))
            .then_with(|| other.selected.cmp(&self.selected))
    }
}

impl PartialOrd for FrontierState {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub(crate) async fn recompute_instance_upgrade_plan_from_source(
    plan: &mut InstanceUpgradePlan,
    fixed_constraints: &[InstanceUpgradeFixedConstraint],
    selected_kind: InstanceUpgradeSolutionKind,
    source: ReadOnlyUpgradeSource,
    state: &State,
) -> crate::Result<()> {
    let started_at = std::time::Instant::now();
    ensure_upgrade_source_files_match(
        &plan.instance_id,
        &plan.source_files,
        &source.source_files,
    )?;
    let snapshot = source.snapshot;
    let cached_catalog = source.catalog;
    let (_, installed) = snapshot_upgrade_items(&snapshot);
    let root_types = installed
        .iter()
        .filter(|node| !node.auto_dependency && node.migratable)
        .map(|node| (node.key.clone(), node.project_type))
        .collect::<HashMap<_, _>>();
    let fixed = FixedRootConstraints::from_constraints(fixed_constraints);
    let catalog = if fixed_constraints.is_empty() {
        cached_catalog.ok_or_else(|| {
            crate::ErrorKind::OtherError(
                "Upgrade plan source is missing its catalog".to_string(),
            )
        })?
    } else {
        match cached_catalog {
            Some(catalog)
                if catalog_supports_fixed_constraints(&catalog, &fixed) =>
            {
                catalog
            }
            _ => {
                load_upgrade_catalog(
                    &root_types,
                    &installed,
                    &fixed,
                    &plan.target_environment,
                    state,
                )
                .await?
            }
        }
    };
    tracing::debug!(
        elapsed_ms = started_at.elapsed().as_millis(),
        catalog_reused = fixed_constraints.is_empty()
            || catalog_supports_fixed_constraints(&catalog, &fixed),
        "[upgrade-plan-bench] recompute catalog"
    );
    classify_items(
        &mut plan.items,
        &installed,
        &catalog,
        &plan.target_environment,
    );
    tracing::debug!(
        elapsed_ms = started_at.elapsed().as_millis(),
        "[upgrade-plan-bench] recompute classified items"
    );
    let roots = roots_from_items(&plan.items, &installed);
    let outcome = solve_upgrade_with_fixed_roots(
        &roots,
        &installed,
        &catalog,
        &fixed,
        &confirmed_prereleases(&plan.items),
    );
    tracing::debug!(
        elapsed_ms = started_at.elapsed().as_millis(),
        "[upgrade-plan-bench] recompute solver"
    );
    apply_solver_issues_to_items(&mut plan.items, &outcome.issues);
    let mut blocking_issues = outcome.issues;
    for item in &plan.items {
        let fixed_prerelease = fixed.contains_content(&item.content_id);
        if let Some(issue) = blocking_issue_for_item(item, fixed_prerelease) {
            blocking_issues.push(issue);
        }
    }
    deduplicate_issues(&mut blocking_issues);
    plan.warnings = item_warnings_with_fixed(
        &plan.items,
        &fixed.by_content_id.keys().cloned().collect::<HashSet<_>>(),
    );
    plan.blocking_issues = blocking_issues;
    plan.newest_solution = outcome
        .solutions
        .iter()
        .max_by(|left, right| compare_newest(left, right, &roots))
        .map(|solution| {
            materialize_solution(
                InstanceUpgradeSolutionKind::Newest,
                solution,
                &roots,
                &installed,
            )
        });
    plan.minimal_change_solution = outcome
        .solutions
        .iter()
        .min_by(|left, right| compare_minimal(left, right, &roots, &installed))
        .map(|solution| {
            materialize_solution(
                InstanceUpgradeSolutionKind::MinimalChange,
                solution,
                &roots,
                &installed,
            )
        });
    plan.selected_solution = match selected_kind {
        InstanceUpgradeSolutionKind::Newest => plan.newest_solution.clone(),
        InstanceUpgradeSolutionKind::MinimalChange => {
            plan.minimal_change_solution.clone()
        }
        InstanceUpgradeSolutionKind::Custom => {
            outcome.solutions.first().map(|solution| {
                materialize_solution(
                    InstanceUpgradeSolutionKind::Custom,
                    solution,
                    &roots,
                    &installed,
                )
            })
        }
    };
    plan.dependency_changes = plan
        .selected_solution
        .as_ref()
        .map(|solution| solution.dependency_changes.clone())
        .unwrap_or_default();
    Ok(())
}

fn catalog_supports_fixed_constraints(
    catalog: &UpgradeCatalog,
    fixed: &FixedRootConstraints,
) -> bool {
    fixed.versions_by_project.iter().all(|(key, versions)| {
        catalog.get(key).is_some_and(|pool| {
            versions.iter().all(|version_id| {
                pool.candidates.iter().any(|candidate| {
                    candidate.version_id == *version_id && candidate.compatible
                })
            })
        })
    })
}

async fn load_upgrade_catalog(
    root_types: &HashMap<NodeKey, ProjectType>,
    installed: &[InstalledNode],
    fixed: &FixedRootConstraints,
    target: &InstanceUpgradeEnvironment,
    state: &State,
) -> crate::Result<UpgradeCatalog> {
    let current_versions = installed
        .iter()
        .flat_map(|node| {
            node.aliases.iter().map(|alias| {
                (alias.key.clone(), alias.current_release_id.clone())
            })
        })
        .collect::<HashMap<_, _>>();
    let mut catalog = HashMap::new();
    let mut queue = root_types
        .iter()
        .map(|(key, project_type)| (key.clone(), *project_type))
        .collect::<VecDeque<_>>();
    let mut seen = HashSet::new();
    let mut exact_versions = fixed.versions_by_project.clone();
    while !queue.is_empty() {
        let mut batch = Vec::new();
        while let Some((key, project_type)) = queue.pop_front() {
            if seen.insert(key.clone()) {
                batch.push((key, project_type));
            }
        }
        let results = futures::future::try_join_all(batch.into_iter().map(
            |(key, project_type)| {
                let current = current_versions.get(&key).cloned();
                let exact =
                    exact_versions.get(&key).cloned().unwrap_or_default();
                let custom_fixed_versions = fixed
                    .versions_for_project(&key)
                    .cloned()
                    .unwrap_or_default();
                async move {
                    let candidates = match key.provider {
                        ContentProvider::Modrinth => {
                            load_modrinth_candidates(
                                &key,
                                project_type,
                                current.as_deref(),
                                &exact,
                                &custom_fixed_versions,
                                target,
                                state,
                            )
                            .await?
                        }
                        ContentProvider::CurseForge => {
                            load_curseforge_candidates(
                                &key,
                                project_type,
                                current.as_deref(),
                                &exact,
                                &custom_fixed_versions,
                                target,
                                state,
                            )
                            .await?
                        }
                        ContentProvider::McArchive | ContentProvider::Local => {
                            CandidatePool::default()
                        }
                    };
                    Ok::<_, crate::Error>((key, candidates))
                }
            },
        ))
        .await?;
        for (key, candidates) in results {
            for candidate in &candidates.candidates {
                for dependency in &candidate.dependencies {
                    if dependency.kind == CandidateDependencyKind::Required {
                        if let Some(version_id) = dependency.version_id.as_ref()
                        {
                            let added = exact_versions
                                .entry(dependency.key.clone())
                                .or_default()
                                .insert(version_id.clone());
                            if added {
                                seen.remove(&dependency.key);
                            }
                        }
                        queue.push_back((
                            dependency.key.clone(),
                            ProjectType::Mod,
                        ));
                    }
                }
            }
            catalog.insert(key, candidates);
        }
    }
    for node in installed {
        for alias in &node.aliases {
            let pool = catalog.entry(alias.key.clone()).or_default();
            if let Some(candidate) =
                pool.candidates.iter_mut().find(|candidate| {
                    candidate.version_id == alias.current_release_id
                })
            {
                candidate.installed_current = true;
            } else {
                pool.candidates.push(UpgradeCandidate {
                    key: alias.key.clone(),
                    version_id: alias.current_release_id.clone(),
                    published: DateTime::<Utc>::MIN_UTC,
                    channel: CandidateChannel::Release,
                    compatible: false,
                    installed_current: true,
                    dependencies: Vec::new(),
                });
            }
        }
    }
    Ok(catalog)
}

/// The environment solver reads provider protocol projections from the shared
/// resource catalog. Source failure remains visible instead of becoming an
/// empty list or an apparently fresh legacy cache hit.
async fn load_planner_project_versions(
    project_id: &ModrinthProjectId,
    target: &InstanceUpgradeEnvironment,
    state: &State,
) -> crate::Result<(Vec<Version>, bool)> {
    crate::resources::legacy_catalog::page(
        crate::resources::SourceId::modrinth(),
        project_id.as_str().into(),
        crate::resources::ipc::ResourceTarget {
            game_versions: vec![target.game_version.clone()],
            loaders: vec![],
            channels: vec![],
        },
        state,
    )
    .await
}
async fn planner_version(
    id: &str,
    state: &State,
) -> crate::Result<Option<Version>> {
    crate::resources::legacy_catalog::exact(
        crate::resources::SourceId::modrinth(),
        None,
        id.into(),
        state,
    )
    .await
}
async fn planner_cf_file(
    project_id: u32,
    file_id: u32,
    state: &State,
) -> crate::Result<Option<crate::api::curseforge::CurseForgeFile>> {
    crate::resources::legacy_catalog::exact(
        crate::resources::SourceId::curseforge(),
        Some(project_id.to_string()),
        file_id.to_string(),
        state,
    )
    .await
}

async fn load_modrinth_candidates(
    key: &NodeKey,
    project_type: ProjectType,
    current_release_id: Option<&str>,
    exact_versions: &HashSet<String>,
    custom_fixed_versions: &HashSet<String>,
    target: &InstanceUpgradeEnvironment,
    state: &State,
) -> crate::Result<CandidatePool> {
    let project_id = ModrinthProjectId::new(key.project_id.clone())?;
    let (mut versions, more_releases) =
        load_planner_project_versions(&project_id, target, state).await?;
    let has_target_game_version_release = versions.iter().any(|version| {
        version
            .game_versions
            .iter()
            .any(|game_version| game_version == &target.game_version)
    });
    versions.sort_by(compare_modrinth_version);
    let (mut selected, exploration_limited) =
        bounded_compatible_candidates(&versions, |version| {
            modrinth_version_matches(version, project_type, target)
        });
    if let Some(current_release_id) = current_release_id
        && !selected
            .iter()
            .any(|version| version.id == current_release_id)
    {
        let current = versions
            .iter()
            .find(|version| version.id == current_release_id)
            .cloned()
            .or(planner_version(current_release_id, state).await?);
        if let Some(current) = current {
            selected.push(current);
        }
    }
    for exact_version in exact_versions {
        let already_selected =
            selected.iter().any(|version| version.id == *exact_version);
        if already_selected && !custom_fixed_versions.contains(exact_version) {
            continue;
        }
        let exact = selected
            .iter()
            .find(|version| version.id == *exact_version)
            .cloned()
            .or_else(|| {
                versions
                    .iter()
                    .find(|version| version.id == *exact_version)
                    .cloned()
            })
            .or(planner_version(&(exact_version.clone()), state).await?);
        if let Some(exact) = exact {
            if custom_fixed_versions.contains(exact_version) {
                validate_modrinth_custom_fixed(
                    key,
                    &exact,
                    project_type,
                    target,
                )?;
            } else if exact.project_id != key.project_id {
                continue;
            }
            if !already_selected {
                selected.push(exact);
            }
        } else if custom_fixed_versions.contains(exact_version) {
            return Err(crate::ErrorKind::InputError(format!(
                "Unknown custom fixed Modrinth version {exact_version}"
            ))
            .into());
        }
    }

    let mut candidates = Vec::new();
    for version in selected {
        let installed_current = current_release_id == Some(version.id.as_str());
        let mut dependencies = Vec::new();
        for dependency in &version.dependencies {
            let project_id = match dependency.project_id.clone() {
                Some(project_id) => project_id,
                None => match dependency.version_id.as_deref() {
                    Some(version_id) => planner_version(version_id, state)
                        .await?
                        .map(|version| version.project_id)
                        .unwrap_or_else(|| {
                            format!("missing-version:{version_id}")
                        }),
                    None => continue,
                },
            };
            dependencies.push(CandidateDependency {
                key: NodeKey::new(ContentProvider::Modrinth, project_id),
                version_id: dependency.version_id.clone(),
                kind: match dependency.dependency_type {
                    DependencyType::Required => {
                        CandidateDependencyKind::Required
                    }
                    DependencyType::Optional => {
                        CandidateDependencyKind::Optional
                    }
                    DependencyType::Incompatible => {
                        CandidateDependencyKind::Incompatible
                    }
                    DependencyType::Embedded => {
                        CandidateDependencyKind::Embedded
                    }
                },
            });
        }
        let compatible =
            modrinth_version_matches(&version, project_type, target);
        candidates.push(UpgradeCandidate {
            key: key.clone(),
            version_id: version.id,
            published: version.date_published,
            channel: modrinth_channel(&version.version_type),
            compatible,
            installed_current,
            dependencies,
        });
    }
    sort_candidates(&mut candidates);
    Ok(CandidatePool {
        candidates,
        exploration_limited: exploration_limited || more_releases,
        has_target_game_version_release,
    })
}

fn validate_modrinth_custom_fixed(
    key: &NodeKey,
    version: &Version,
    project_type: ProjectType,
    target: &InstanceUpgradeEnvironment,
) -> crate::Result<()> {
    if version.project_id != key.project_id {
        return Err(crate::ErrorKind::InputError(format!(
            "Custom fixed Modrinth version {} belongs to project {}, not {}",
            version.id, version.project_id, key.project_id
        ))
        .into());
    }
    if !modrinth_version_matches(version, project_type, target) {
        return Err(crate::ErrorKind::InputError(format!(
            "Custom fixed Modrinth version {} is not compatible with the target environment",
            version.id
        ))
        .into());
    }
    Ok(())
}

async fn load_curseforge_candidates(
    key: &NodeKey,
    project_type: ProjectType,
    current_release_id: Option<&str>,
    exact_versions: &HashSet<String>,
    custom_fixed_versions: &HashSet<String>,
    target: &InstanceUpgradeEnvironment,
    state: &State,
) -> crate::Result<CandidatePool> {
    let project_id = key.project_id.parse::<u32>().map_err(|_| {
        crate::ErrorKind::InputError(format!(
            "Invalid CurseForge project ID {}",
            key.project_id
        ))
    })?;
    let (mut files, more_releases): (
        Vec<crate::api::curseforge::CurseForgeFile>,
        bool,
    ) = crate::resources::legacy_catalog::page(
        crate::resources::SourceId::curseforge(),
        project_id.to_string(),
        crate::resources::ipc::ResourceTarget {
            game_versions: vec![target.game_version.clone()],
            loaders: if project_type == ProjectType::Mod {
                match target.mod_loader.as_str() {
                    "forge" | "fabric" | "quilt" | "neoforge" => {
                        vec![target.mod_loader.as_str().into()]
                    }
                    "lite_loader" => vec!["liteloader".into()],
                    _ => vec![],
                }
            } else {
                vec![]
            },
            channels: vec![],
        },
        state,
    )
    .await?;
    let has_target_game_version_release = files.iter().any(|file| {
        file.is_available && curseforge_game_version_matches(file, target)
    });
    files.sort_by(|left, right| {
        curseforge_channel(right.release_type)
            .rank()
            .cmp(&curseforge_channel(left.release_type).rank())
            .then_with(|| right.file_date.cmp(&left.file_date))
            .then_with(|| right.id.cmp(&left.id))
    });
    let (mut selected, exploration_limited) =
        bounded_compatible_candidates(&files, |file| {
            file.is_available
                && curseforge_file_matches(file, project_type, target)
        });
    if let Some(current_release_id) = current_release_id
        && !selected
            .iter()
            .any(|file| file.id.to_string() == current_release_id)
    {
        let current = files
            .iter()
            .find(|file| file.id.to_string() == current_release_id)
            .cloned();
        let current = match current {
            Some(current) => Some(current),
            None => match current_release_id.parse::<u32>() {
                Ok(file_id) => {
                    planner_cf_file(project_id, file_id, state).await?
                }
                Err(_) => None,
            },
        };
        if let Some(current) = current {
            selected.push(current);
        }
    }
    for exact_version in exact_versions {
        let already_selected = selected
            .iter()
            .any(|file| file.id.to_string() == *exact_version);
        if already_selected && !custom_fixed_versions.contains(exact_version) {
            continue;
        }
        let file_id = exact_version.parse::<u32>().map_err(|_| {
            crate::ErrorKind::InputError(format!(
                "Invalid CurseForge file ID {exact_version}"
            ))
        })?;
        let exact =
            match selected.iter().find(|file| file.id == file_id).cloned() {
                Some(file) => file,
                None => planner_cf_file(project_id, file_id, state)
                    .await?
                    .ok_or_else(|| {
                        crate::ErrorKind::InputError(format!(
                            "Unknown CurseForge file ID {exact_version}"
                        ))
                    })?,
            };
        if exact.mod_id != project_id {
            return Err(crate::ErrorKind::InputError(format!(
                "Fixed or exact CurseForge file {exact_version} belongs to project {}, not {project_id}",
                exact.mod_id
            ))
            .into());
        }
        if custom_fixed_versions.contains(exact_version)
            && !curseforge_file_matches(&exact, project_type, target)
        {
            return Err(crate::ErrorKind::InputError(format!(
                "Custom fixed CurseForge file {exact_version} is not compatible with the target environment"
            ))
            .into());
        }
        if !already_selected {
            selected.push(exact);
        }
    }
    let mut candidates = selected
        .into_iter()
        .map(|file| {
            let installed_current = current_release_id
                .is_some_and(|current| current == file.id.to_string());
            UpgradeCandidate {
                key: key.clone(),
                version_id: file.id.to_string(),
                published: DateTime::parse_from_rfc3339(&file.file_date)
                    .map(|date| date.with_timezone(&Utc))
                    .unwrap_or(DateTime::<Utc>::MIN_UTC),
                channel: curseforge_channel(file.release_type),
                compatible: curseforge_file_matches(
                    &file,
                    project_type,
                    target,
                ),
                installed_current,
                dependencies: file
                    .dependencies
                    .into_iter()
                    .filter_map(|dependency| {
                        let kind = match dependency.relation() {
                            DependencyRelation::Required
                            | DependencyRelation::Include => {
                                CandidateDependencyKind::Required
                            }
                            DependencyRelation::Optional => {
                                CandidateDependencyKind::Optional
                            }
                            DependencyRelation::Incompatible => {
                                CandidateDependencyKind::Incompatible
                            }
                            DependencyRelation::Embedded
                            | DependencyRelation::Tool => {
                                CandidateDependencyKind::Embedded
                            }
                            DependencyRelation::Unknown(_) => return None,
                        };
                        Some(CandidateDependency {
                            key: NodeKey::new(
                                ContentProvider::CurseForge,
                                dependency.mod_id.to_string(),
                            ),
                            version_id: None,
                            kind,
                        })
                    })
                    .collect(),
            }
        })
        .collect::<Vec<_>>();
    sort_candidates(&mut candidates);
    Ok(CandidatePool {
        candidates,
        exploration_limited: exploration_limited || more_releases,
        has_target_game_version_release,
    })
}

fn bounded_compatible_candidates<T: Clone>(
    candidates: &[T],
    mut compatible: impl FnMut(&T) -> bool,
) -> (Vec<T>, bool) {
    let mut selected = Vec::new();
    let mut compatible_count = 0;
    for candidate in candidates {
        if !compatible(candidate) {
            continue;
        }
        compatible_count += 1;
        if selected.len() < MAX_CANDIDATES_PER_PROJECT {
            selected.push(candidate.clone());
        }
    }
    (selected, compatible_count > MAX_CANDIDATES_PER_PROJECT)
}

fn compare_modrinth_version(left: &Version, right: &Version) -> Ordering {
    modrinth_channel(&right.version_type)
        .rank()
        .cmp(&modrinth_channel(&left.version_type).rank())
        .then_with(|| right.date_published.cmp(&left.date_published))
        .then_with(|| right.id.cmp(&left.id))
}

fn modrinth_channel(version_type: &str) -> CandidateChannel {
    if version_type.eq_ignore_ascii_case("beta") {
        CandidateChannel::Beta
    } else if version_type.eq_ignore_ascii_case("alpha") {
        CandidateChannel::Alpha
    } else {
        CandidateChannel::Release
    }
}

fn curseforge_channel(release_type: u32) -> CandidateChannel {
    match release_type {
        1 => CandidateChannel::Release,
        2 => CandidateChannel::Beta,
        _ => CandidateChannel::Alpha,
    }
}

fn sort_candidates(candidates: &mut [UpgradeCandidate]) {
    candidates.sort_by(|left, right| {
        right
            .channel
            .rank()
            .cmp(&left.channel.rank())
            .then_with(|| right.published.cmp(&left.published))
            .then_with(|| right.version_id.cmp(&left.version_id))
    });
}

fn modrinth_version_matches(
    version: &Version,
    project_type: ProjectType,
    target: &InstanceUpgradeEnvironment,
) -> bool {
    if !version
        .game_versions
        .iter()
        .any(|game_version| game_version == &target.game_version)
    {
        return false;
    }
    match project_type {
        ProjectType::Mod => version.loaders.iter().any(|loader| {
            loader.eq_ignore_ascii_case(target.mod_loader.as_str())
        }),
        ProjectType::ShaderPack => shader_loader_matches(
            version.loaders.iter().map(String::as_str),
            target.shader_runtime,
        ),
        ProjectType::DataPack => version
            .loaders
            .iter()
            .any(|loader| loader.eq_ignore_ascii_case("datapack")),
        ProjectType::ResourcePack => version.loaders.iter().any(|loader| {
            loader.eq_ignore_ascii_case("minecraft")
                || loader.eq_ignore_ascii_case("vanilla")
        }),
        ProjectType::Schematic | ProjectType::WorldSave => false,
    }
}

fn curseforge_file_matches(
    file: &crate::api::curseforge::CurseForgeFile,
    project_type: ProjectType,
    target: &InstanceUpgradeEnvironment,
) -> bool {
    let game_version_matches = curseforge_game_version_matches(file, target);
    if !game_version_matches {
        return false;
    }
    match project_type {
        ProjectType::Mod => file.game_versions.iter().any(|value| {
            value.eq_ignore_ascii_case(target.mod_loader.as_str())
        }),
        ProjectType::ShaderPack => shader_loader_matches(
            file.game_versions.iter().map(String::as_str),
            target.shader_runtime,
        ),
        ProjectType::DataPack | ProjectType::ResourcePack => true,
        ProjectType::Schematic | ProjectType::WorldSave => false,
    }
}

fn curseforge_game_version_matches(
    file: &crate::api::curseforge::CurseForgeFile,
    target: &InstanceUpgradeEnvironment,
) -> bool {
    file.game_versions
        .iter()
        .any(|value| value == &target.game_version)
        || file.sortable_game_versions.iter().any(|value| {
            value.game_version.as_deref() == Some(&target.game_version)
                || value.game_version_name == target.game_version
        })
}

fn shader_loader_matches<'a>(
    loaders: impl Iterator<Item = &'a str>,
    runtime: ShaderRuntime,
) -> bool {
    let expected = match runtime {
        ShaderRuntime::Iris => "iris",
        ShaderRuntime::OptiFine => "optifine",
        ShaderRuntime::None | ShaderRuntime::Unknown => return false,
    };
    loaders.into_iter().any(|loader| {
        loader.eq_ignore_ascii_case(expected)
            || runtime == ShaderRuntime::OptiFine
                && loader.eq_ignore_ascii_case("optifine")
    })
}

fn classify_items(
    items: &mut [InstanceUpgradeItem],
    installed: &[InstalledNode],
    catalog: &UpgradeCatalog,
    target: &InstanceUpgradeEnvironment,
) {
    for item in items {
        if matches!(
            item.status,
            InstanceUpgradeItemStatus::Unidentified
                | InstanceUpgradeItemStatus::UnsupportedContentType
        ) {
            continue;
        }
        if item.project_type == ProjectType::ShaderPack {
            match target.shader_runtime {
                ShaderRuntime::None => {
                    item.status =
                        InstanceUpgradeItemStatus::ShaderRuntimeMissing;
                    if item.resolution.action == InstanceUpgradeAction::Upgrade
                    {
                        item.resolution.action = InstanceUpgradeAction::Keep;
                    }
                    continue;
                }
                ShaderRuntime::Unknown => {
                    item.status =
                        InstanceUpgradeItemStatus::ShaderRuntimeUnknown;
                    if item.resolution.action == InstanceUpgradeAction::Upgrade
                    {
                        item.resolution.action = InstanceUpgradeAction::Keep;
                    }
                    continue;
                }
                ShaderRuntime::Iris | ShaderRuntime::OptiFine => {}
            }
        }
        let Some(node) = installed
            .iter()
            .find(|node| node.content_id == item.content_id)
        else {
            continue;
        };
        let pool = catalog.get(&node.key);
        let candidates =
            pool.map(|pool| pool.candidates.as_slice()).unwrap_or(&[]);
        let compatible = candidates
            .iter()
            .filter(|candidate| candidate.compatible)
            .collect::<Vec<_>>();
        item.candidate_release_ids = compatible
            .iter()
            .map(|candidate| candidate.version_id.clone())
            .collect();
        let has_stable = compatible
            .iter()
            .any(|candidate| candidate.channel == CandidateChannel::Release);
        let current_compatible = compatible
            .iter()
            .any(|candidate| candidate.version_id == node.current_release_id);
        item.status = if has_stable {
            if current_compatible {
                InstanceUpgradeItemStatus::AlreadyCompatible
            } else {
                InstanceUpgradeItemStatus::UpgradeAvailable
            }
        } else if !compatible.is_empty() {
            InstanceUpgradeItemStatus::PrereleaseOnly
        } else if item.project_type == ProjectType::ShaderPack
            && pool.is_some_and(|pool| pool.has_target_game_version_release)
        {
            InstanceUpgradeItemStatus::NoCompatibleShaderRuntime
        } else {
            InstanceUpgradeItemStatus::NoCompatibleRelease
        };
    }
}

fn roots_from_items(
    items: &[InstanceUpgradeItem],
    installed: &[InstalledNode],
) -> Vec<RootRequest> {
    items
        .iter()
        .filter_map(|item| {
            let node = installed.iter().find(|node| {
                node.content_id == item.content_id
                    && !node.auto_dependency
                    && node.migratable
            })?;
            Some(RootRequest {
                content_id: item.content_id.clone(),
                key: node.key.clone(),
                current_release_id: node.current_release_id.clone(),
                enabled: node.enabled,
                action: item.resolution.action,
                allow_prerelease: item.resolution.allow_prerelease,
            })
        })
        .collect()
}

fn confirmed_prereleases(
    items: &[InstanceUpgradeItem],
) -> HashSet<(NodeKey, String)> {
    items
        .iter()
        .flat_map(|item| {
            item.resolution.confirmed_prerelease_dependencies.iter()
        })
        .map(|confirmation| {
            (
                NodeKey::new(confirmation.provider, &confirmation.project_id),
                confirmation.version_id.clone(),
            )
        })
        .collect()
}

fn materialize_solution(
    kind: InstanceUpgradeSolutionKind,
    solution: &SolverResult,
    roots: &[RootRequest],
    installed: &[InstalledNode],
) -> InstanceUpgradeSolution {
    let aliases = InstalledAliasIndex::new(installed);
    let root_keys = roots
        .iter()
        .map(|root| root.key.clone())
        .collect::<HashSet<_>>();
    let enabled = solution_enabled_nodes(solution, roots, &aliases);
    let installed_by_key = installed
        .iter()
        .flat_map(|node| {
            node.aliases.iter().map(move |alias| {
                (alias.key.clone(), (node, alias.current_release_id.as_str()))
            })
        })
        .collect::<HashMap<_, _>>();
    let mut selections = roots
        .iter()
        .map(|root| {
            let candidate = solution.assignments.get(&root.key);
            let target_release_id =
                candidate.map(|value| value.version_id.clone());
            let action = if root.action == InstanceUpgradeAction::Disable {
                InstanceUpgradeAction::Disable
            } else if target_release_id.as_deref()
                == Some(root.current_release_id.as_str())
            {
                InstanceUpgradeAction::Keep
            } else {
                InstanceUpgradeAction::Upgrade
            };
            InstanceUpgradeSelection {
                content_id: root.content_id.clone(),
                provider: Some(root.key.provider),
                project_id: Some(root.key.project_id.clone()),
                current_release_id: Some(root.current_release_id.clone()),
                target_release_id,
                action,
                enabled: root.enabled
                    && action != InstanceUpgradeAction::Disable,
            }
        })
        .collect::<Vec<_>>();
    selections.sort_by(|left, right| left.content_id.cmp(&right.content_id));

    let mut dependency_changes = solution
        .assignments
        .iter()
        .filter(|(key, _)| !root_keys.contains(*key))
        .map(|(key, candidate)| {
            let current_release_id = installed_by_key
                .get(key)
                .map(|(_, release_id)| (*release_id).to_string());
            let kind = match current_release_id.as_deref() {
                None => InstanceUpgradeDependencyChangeKind::Add,
                Some(current) if current == candidate.version_id => {
                    InstanceUpgradeDependencyChangeKind::Keep
                }
                Some(_) => InstanceUpgradeDependencyChangeKind::Upgrade,
            };
            InstanceUpgradeDependencyChange {
                existing_content_id: installed_by_key
                    .get(key)
                    .map(|(node, _)| node.content_id.clone()),
                provider: key.provider,
                project_id: key.project_id.clone(),
                current_release_id,
                target_release_id: Some(candidate.version_id.clone()),
                kind,
                enabled: key_is_enabled(key, &enabled, &aliases),
            }
        })
        .collect::<Vec<_>>();
    for node in installed
        .iter()
        .filter(|node| node.auto_dependency && node.migratable)
    {
        if node
            .aliases
            .iter()
            .any(|alias| solution.assignments.contains_key(&alias.key))
            || installed
                .iter()
                .any(|other| other.key == node.key && other.user_owned)
        {
            continue;
        }
        dependency_changes.push(InstanceUpgradeDependencyChange {
            existing_content_id: Some(node.content_id.clone()),
            provider: node.key.provider,
            project_id: node.key.project_id.clone(),
            current_release_id: Some(node.current_release_id.clone()),
            target_release_id: None,
            kind: InstanceUpgradeDependencyChangeKind::Remove,
            enabled: false,
        });
    }
    dependency_changes.sort_by(|left, right| {
        left.provider
            .as_str()
            .cmp(right.provider.as_str())
            .then_with(|| left.project_id.cmp(&right.project_id))
    });
    let mut warnings = roots
        .iter()
        .filter_map(|root| {
            let candidate = solution.assignments.get(&root.key)?;
            (root.action == InstanceUpgradeAction::Keep
                && !candidate.compatible)
                .then(|| {
                    issue(
                        InstanceUpgradeIssueCode::KeepIncompatible,
                        format!(
                            "{} remains incompatible with target environment",
                            root.key.label()
                        ),
                        Some(&root.key),
                        Some(&root.key.project_id),
                        None,
                    )
                })
        })
        .collect::<Vec<_>>();
    for key in &solution.preserved_unsafe {
        let Some(candidate) = solution.assignments.get(key) else {
            continue;
        };
        if candidate.compatible
            || !key_is_enabled(key, &enabled, &aliases)
            || warnings.iter().any(|warning| {
                warning.provider == Some(key.provider)
                    && warning.project_id.as_deref()
                        == Some(key.project_id.as_str())
            })
        {
            continue;
        }
        warnings.push(issue(
            InstanceUpgradeIssueCode::KeepIncompatible,
            format!(
                "{} is preserved despite target incompatibility",
                key.label()
            ),
            Some(key),
            Some(&key.project_id),
            None,
        ));
    }
    for (key, candidate) in &solution.assignments {
        if !candidate.channel.is_prerelease()
            || solution.preserved_unsafe.contains(key)
        {
            continue;
        }
        warnings.push(issue(
            InstanceUpgradeIssueCode::PrereleaseOnly,
            format!(
                "{} uses explicitly confirmed prerelease {}",
                key.label(),
                candidate.version_id
            ),
            Some(key),
            Some(&key.project_id),
            None,
        ));
    }
    InstanceUpgradeSolution {
        kind,
        selections,
        dependency_changes,
        warnings,
    }
}

fn solution_enabled_nodes(
    solution: &SolverResult,
    roots: &[RootRequest],
    aliases: &InstalledAliasIndex,
) -> HashSet<NodeKey> {
    let mut enabled = HashSet::new();
    let mut queue = roots
        .iter()
        .filter(|root| {
            root.enabled && root.action != InstanceUpgradeAction::Disable
        })
        .map(|root| root.key.clone())
        .collect::<VecDeque<_>>();
    while let Some(key) = queue.pop_front() {
        if !enabled.insert(key.clone()) {
            continue;
        }
        let assigned_key = assigned_key_for_requirement(
            &key,
            &solution.assignments,
            &solution.physical_assignments,
            aliases,
        );
        if let Some(candidate) = assigned_key
            .and_then(|assigned_key| solution.assignments.get(assigned_key))
        {
            for dependency in &candidate.dependencies {
                if dependency.kind == CandidateDependencyKind::Required {
                    queue.push_back(dependency.key.clone());
                }
            }
        }
    }
    enabled
}

fn key_is_enabled(
    key: &NodeKey,
    enabled: &HashSet<NodeKey>,
    aliases: &InstalledAliasIndex,
) -> bool {
    enabled.contains(key)
        || aliases.content_id(key).is_some_and(|content_id| {
            aliases.aliases_by_content_id.get(content_id).is_some_and(
                |physical_aliases| {
                    physical_aliases.keys().any(|alias| enabled.contains(alias))
                },
            )
        })
}

fn blocking_issue_for_item(
    item: &InstanceUpgradeItem,
    fixed_prerelease: bool,
) -> Option<InstanceUpgradeIssue> {
    if item.resolution.action != InstanceUpgradeAction::Upgrade {
        return None;
    }
    let code = match item.status {
        InstanceUpgradeItemStatus::PrereleaseOnly
            if !item.resolution.allow_prerelease && !fixed_prerelease =>
        {
            InstanceUpgradeIssueCode::PrereleaseOnly
        }
        InstanceUpgradeItemStatus::NoCompatibleRelease => {
            InstanceUpgradeIssueCode::NoCompatibleRelease
        }
        InstanceUpgradeItemStatus::NoCompatibleShaderRuntime => {
            InstanceUpgradeIssueCode::NoCompatibleShaderRuntime
        }
        InstanceUpgradeItemStatus::ShaderRuntimeMissing => {
            InstanceUpgradeIssueCode::ShaderRuntimeMissing
        }
        InstanceUpgradeItemStatus::ShaderRuntimeUnknown => {
            InstanceUpgradeIssueCode::ShaderRuntimeUnknown
        }
        _ => return None,
    };
    Some(InstanceUpgradeIssue {
        code,
        message: format!(
            "{} cannot be upgraded without user resolution",
            item.relative_path
        ),
        content_id: Some(item.content_id.clone()),
        provider: item.provider,
        project_id: item.project_id.clone(),
        conflicting_project_id: None,
        dependency_requirements: Vec::new(),
    })
}

fn item_warnings(items: &[InstanceUpgradeItem]) -> Vec<InstanceUpgradeIssue> {
    item_warnings_with_fixed(items, &HashSet::new())
}

fn item_warnings_with_fixed(
    items: &[InstanceUpgradeItem],
    fixed_content_ids: &HashSet<String>,
) -> Vec<InstanceUpgradeIssue> {
    items
        .iter()
        .filter_map(|item| {
            let code = match item.status {
                InstanceUpgradeItemStatus::Unidentified => {
                    InstanceUpgradeIssueCode::Unidentified
                }
                InstanceUpgradeItemStatus::UnsupportedContentType => {
                    InstanceUpgradeIssueCode::UnsupportedContentType
                }
                InstanceUpgradeItemStatus::PrereleaseOnly => {
                    InstanceUpgradeIssueCode::PrereleaseOnly
                }
                InstanceUpgradeItemStatus::NoCompatibleRelease
                    if item.resolution.action
                        == InstanceUpgradeAction::Keep =>
                {
                    InstanceUpgradeIssueCode::KeepIncompatible
                }
                InstanceUpgradeItemStatus::ShaderRuntimeMissing
                    if item.resolution.action
                        == InstanceUpgradeAction::Keep =>
                {
                    InstanceUpgradeIssueCode::ShaderRuntimeMissing
                }
                InstanceUpgradeItemStatus::ShaderRuntimeUnknown
                    if item.resolution.action
                        == InstanceUpgradeAction::Keep =>
                {
                    InstanceUpgradeIssueCode::ShaderRuntimeUnknown
                }
                _ => return None,
            };
            let message = match code {
                InstanceUpgradeIssueCode::PrereleaseOnly
                    if item.resolution.allow_prerelease
                        || fixed_content_ids.contains(&item.content_id) =>
                {
                    format!(
                        "{} uses a confirmed prerelease",
                        item.relative_path
                    )
                }
                InstanceUpgradeIssueCode::PrereleaseOnly => format!(
                    "{} requires explicit prerelease confirmation",
                    item.relative_path
                ),
                _ => format!("{} will be preserved", item.relative_path),
            };
            Some(InstanceUpgradeIssue {
                code,
                message,
                content_id: Some(item.content_id.clone()),
                provider: item.provider,
                project_id: item.project_id.clone(),
                conflicting_project_id: None,
                dependency_requirements: Vec::new(),
            })
        })
        .collect()
}

fn apply_solver_issues_to_items(
    items: &mut [InstanceUpgradeItem],
    issues: &[InstanceUpgradeIssue],
) {
    for issue in issues {
        let status = match issue.code {
            InstanceUpgradeIssueCode::DependencyConflict => {
                InstanceUpgradeItemStatus::DependencyConflict
            }
            InstanceUpgradeIssueCode::MissingRequiredDependency => {
                InstanceUpgradeItemStatus::MissingRequiredDependency
            }
            InstanceUpgradeIssueCode::IncompatibleDependency => {
                InstanceUpgradeItemStatus::IncompatibleDependency
            }
            _ => continue,
        };
        for item in items.iter_mut().filter(|item| {
            issue.project_id.as_deref() == item.project_id.as_deref()
                || issue.conflicting_project_id.as_deref()
                    == item.project_id.as_deref()
                || issue.dependency_requirements.iter().any(|requirement| {
                    requirement.root_content_id == item.content_id
                })
        }) {
            item.status = status;
        }
    }
}

fn issue(
    code: InstanceUpgradeIssueCode,
    message: impl Into<String>,
    key: Option<&NodeKey>,
    project_id: Option<&str>,
    conflicting_project_id: Option<&str>,
) -> InstanceUpgradeIssue {
    InstanceUpgradeIssue {
        code,
        message: message.into(),
        content_id: None,
        provider: key.map(|key| key.provider),
        project_id: project_id.map(str::to_string),
        conflicting_project_id: conflicting_project_id.map(str::to_string),
        dependency_requirements: Vec::new(),
    }
}

fn issue_with_requirements(
    code: InstanceUpgradeIssueCode,
    message: impl Into<String>,
    key: Option<&NodeKey>,
    project_id: Option<&str>,
    conflicting_project_id: Option<&str>,
    dependency_requirements: Vec<InstanceUpgradeDependencyRequirement>,
) -> InstanceUpgradeIssue {
    InstanceUpgradeIssue {
        dependency_requirements,
        ..issue(code, message, key, project_id, conflicting_project_id)
    }
}

fn deduplicate_issues(issues: &mut Vec<InstanceUpgradeIssue>) {
    let mut seen = HashSet::new();
    issues.retain(|issue| {
        seen.insert((
            format!("{:?}", issue.code),
            issue.content_id.clone(),
            issue.project_id.clone(),
            issue.conflicting_project_id.clone(),
        ))
    });
}

#[cfg(test)]
mod tests;
