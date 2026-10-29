mod dependencies;
mod reconciliation;

use dependencies::backfill_dependency_edges_from_metadata;
use reconciliation::reconcile_curseforge_manual_downloads;
pub(crate) use reconciliation::reconcile_curseforge_members;

use std::collections::{HashMap, HashSet};

use sqlx::Row;

use super::list_content::list_all_content_from_files;
use super::sync_content_files::project_type_for_file;
use crate::State;
#[cfg(test)]
use crate::state::ContentSourceKind;
use crate::state::instances::adapters::{
    filesystem,
    sqlite::{content_rows, instance_rows},
};
use crate::state::instances::{
    ContentDependencyInfo, ContentDependencyRef, ContentItemCapabilities,
    ContentOwnershipKind, InstanceContentPack, InstanceContentSnapshot,
    InstanceContentSnapshotItem, InstanceContentWarning, InstanceLink,
    PackMemberMaterializationState, PackMemberOverrideKind,
};
use crate::state::{
    CacheBehaviour, CachedEntry, ContentItem, ContentItemProject,
    ContentItemVersion, ContentProvider, ContentProviderRef,
};

/// Content operations share one per-instance lock. Waiting for it must be
/// bounded: a stale holder would otherwise pin a background task, or a UI
/// request, for the rest of the session.
const CONTENT_LOCK_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(30);

/// In-flight dependency backfills keyed by instance id and content revision.
///
/// A remote refresh starts a metadata backfill. Switching pages quickly used to
/// stack one scan per visit, so a newer backfill for the same instance now
/// replaces the previous one only when the content revision changed. Repeated
/// refreshes of the same snapshot reuse its existing task. Each registration
/// carries its own generation, so a cancelled task can only retire the registry
/// entry it created and never the one that replaced it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct BackfillVersion {
    content_set_id: String,
    revision: u64,
}

struct BackfillTask {
    version: BackfillVersion,
    generation: u64,
    handle: tokio::task::JoinHandle<()>,
}

static BACKFILL_TASKS: std::sync::LazyLock<
    tokio::sync::Mutex<HashMap<String, BackfillTask>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

static BACKFILL_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Registers a backfill unless the same snapshot already has an active task.
/// A newer content revision replaces and cancels the previous task.
async fn replace_backfill_task(
    instance_id: &str,
    version: BackfillVersion,
    handle: tokio::task::JoinHandle<()>,
) -> Option<u64> {
    let mut tasks = BACKFILL_TASKS.lock().await;
    if tasks.get(instance_id).is_some_and(|task| {
        task.version == version && !task.handle.is_finished()
    }) {
        handle.abort();
        return None;
    }

    let generation =
        BACKFILL_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let previous = tasks.insert(
        instance_id.to_string(),
        BackfillTask {
            version,
            generation,
            handle,
        },
    );
    drop(tasks);
    if let Some(previous) = previous {
        previous.handle.abort();
    }
    Some(generation)
}

/// Drops the registry entry for an instance once the backfill that owns
/// `generation` has finished. A replaced task leaves the newer entry alone.
async fn forget_backfill_task(instance_id: &str, generation: u64) {
    BACKFILL_TASKS.lock().await.retain(|registered_id, task| {
        registered_id != instance_id || task.generation != generation
    });
}

/// Clears the instance entry when its backfill ends, including the early returns
/// that bail out before writing anything.
struct BackfillGuard(String, u64);

impl Drop for BackfillGuard {
    fn drop(&mut self) {
        let key = self.0.clone();
        let generation = self.1;
        // Drop can run outside a runtime (for example while a test shuts down),
        // in which case there is nothing left to clean up.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                forget_backfill_task(&key, generation).await;
            });
        }
    }
}

pub(crate) async fn get_content_snapshot(
    instance_id: &str,
    refresh_remote: bool,
    state: &State,
) -> crate::Result<InstanceContentSnapshot> {
    get_content_snapshot_inner(instance_id, refresh_remote, true, true, state)
        .await
}

pub(crate) async fn get_cached_content_snapshot(
    instance_id: &str,
    state: &State,
) -> crate::Result<InstanceContentSnapshot> {
    get_content_snapshot_inner(instance_id, false, false, true, state).await
}

pub(crate) async fn get_local_content_snapshot(
    instance_id: &str,
    state: &State,
) -> crate::Result<InstanceContentSnapshot> {
    get_content_snapshot_inner(instance_id, false, true, false, state).await
}

async fn get_content_snapshot_inner(
    instance_id: &str,
    refresh_remote: bool,
    sync_direct_instance: bool,
    enrich: bool,
    state: &State,
) -> crate::Result<InstanceContentSnapshot> {
    super::repair_recorded_pack_unlink(instance_id, &state.pool).await?;
    let instance = instance_rows::get_instance_by_id_with_pool(
        instance_id.to_string(),
        state.pool.clone(),
    )
    .await?
    .ok_or_else(|| {
        crate::ErrorKind::InputError("Unknown instance".to_string())
    })?;
    let (content_set, link, cached_files) = {
        let mut connection = state.pool.acquire().await?;
        let content_set = content_rows::get_applied_content_set(
            instance_id,
            &mut *connection,
        )
        .await?
        .ok_or_else(|| {
            crate::ErrorKind::InputError(
                "Instance has no applied content set".to_string(),
            )
        })?;
        let link =
            instance_rows::get_instance_link(instance_id, &mut *connection)
                .await?;
        let files =
            content_rows::get_instance_files(&instance.id, &mut *connection)
                .await?;
        (content_set, link, files)
    };
    // Managed instances keep their cached database snapshot on first open.
    // Directly associated instances have no install pipeline to seed that
    // snapshot, so their first content-page load must scan the external game
    // directory. Later loads reuse the populated rows unless explicitly
    // refreshed.
    let files = if refresh_remote
        || (sync_direct_instance
            && instance.is_direct_linked()
            && cached_files.is_empty())
    {
        let cancel = tokio_util::sync::CancellationToken::new();
        let refreshed = if refresh_remote {
            crate::resources::installed::refresh_snapshot(
                state,
                instance_id,
                &cancel,
            )
            .await
        } else {
            crate::resources::installed::refresh_install_snapshot(
                state,
                instance_id,
                &cancel,
            )
            .await
        };
        refreshed
            .map_err(|error| crate::ErrorKind::OtherError(error.to_string()))?;
        {
            let mut connection = state.pool.acquire().await?;
            content_rows::get_instance_files(&instance.id, &mut *connection)
                .await?
        }
    } else {
        cached_files
    };
    let content_set = if refresh_remote
        || sync_direct_instance && instance.is_direct_linked()
    {
        {
            let mut connection = state.pool.acquire().await?;
            content_rows::get_applied_content_set(instance_id, &mut *connection)
                .await?
        }
        .ok_or_else(|| {
            crate::ErrorKind::InputError(
                "Instance has no applied content set after refresh".into(),
            )
        })?
    } else {
        content_set
    };
    let mut warnings = Vec::new();
    if refresh_remote
        && let InstanceLink::CurseForgeModpack {
            project_id,
            version_id,
        } = &link
    {
        let needs_reconciliation = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(
				SELECT 1 FROM instance_pack_members
				WHERE content_set_id = ? AND reconciled = 0
			)",
        )
        .bind(&content_set.id)
        .fetch_one(&state.pool)
        .await?
            != 0;
        let needs_manual_recovery = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(
				SELECT 1 FROM instance_pack_members member
				WHERE member.content_set_id = ?
					AND member.provider = 'curseforge'
					AND member.required = 1
					AND member.materialization_state = 'missing'
			)",
        )
        .bind(&content_set.id)
        .fetch_one(&state.pool)
        .await?
            != 0;
        let needs_local_classification = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(
                SELECT 1
                FROM instance_files file
                LEFT JOIN instance_content_entries entry
                    ON entry.content_set_id = ? AND entry.file_id = file.id
                WHERE file.instance_id = ? AND file.missing = 0
                    AND entry.id IS NULL
            )",
        )
        .bind(&content_set.id)
        .bind(instance_id)
        .fetch_one(&state.pool)
        .await?
            != 0;
        let parsed_ids = project_id
            .parse::<u32>()
            .ok()
            .zip(version_id.parse::<u32>().ok());
        let result = match parsed_ids {
            Some((project_id, version_id)) => {
                match crate::api::curseforge::get_modpack_expected_members(
                    project_id, version_id,
                )
                .await
                {
                    Ok(expected) => {
                        let should_reconcile = needs_reconciliation
                            || needs_local_classification
                            || !expected.overrides.is_empty();
                        if should_reconcile
                            && let Err(error) = reconcile_curseforge_members(
                                instance_id,
                                &content_set.id,
                                &expected,
                                state,
                            )
                            .await
                        {
                            Err(error)
                        } else if should_reconcile || needs_manual_recovery {
                            reconcile_curseforge_manual_downloads(
                                instance_id,
                                &content_set.id,
                                &expected.members,
                                state,
                            )
                            .await
                        } else {
                            Ok(())
                        }
                    }
                    Err(error) => Err(error),
                }
            }
            None => Err(crate::ErrorKind::InputError(
                "Linked CurseForge pack IDs are invalid".to_string(),
            )
            .into()),
        };
        if let Err(error) = result {
            warnings.push(InstanceContentWarning {
                code: "pack_membership_reconciliation_failed".to_string(),
                message: error.to_string(),
                provider: Some(ContentProvider::CurseForge),
            });
        }
    }

    let entries = {
        let mut connection = state.pool.acquire().await?;
        content_rows::get_content_entries(&content_set.id, &mut *connection)
            .await?
    };
    let members =
        content_rows::get_pack_members(&content_set.id, &state.pool).await?;
    let pending_manual_downloads =
        content_rows::get_pending_manual_downloads(instance_id, &state.pool)
            .await?;

    let mut refs_by_entry = HashMap::new();
    let mut origin_by_entry = HashMap::new();
    let provider_rows = sqlx::query(
        "SELECT ref.content_entry_id, ref.provider,
                ref.provider_project_id, ref.provider_release_id,
				ref.provider_file_id,
                ref.is_origin
         FROM instance_content_provider_refs ref
         INNER JOIN instance_content_entries entry
            ON entry.id = ref.content_entry_id
         WHERE entry.content_set_id = ?
         ORDER BY ref.content_entry_id, ref.is_origin DESC, ref.provider,
            ref.provider_project_id, ref.provider_release_id IS NULL,
            ref.provider_release_id",
    )
    .bind(&content_set.id)
    .fetch_all(&state.pool)
    .await?;
    for row in provider_rows {
        let entry_id = row.try_get::<String, _>("content_entry_id")?;
        let provider = row.try_get::<String, _>("provider")?;
        let provider_project_id =
            row.try_get::<String, _>("provider_project_id")?;
        let provider_release_id =
            row.try_get::<Option<String>, _>("provider_release_id")?;
        let provider_file_id =
            row.try_get::<Option<String>, _>("provider_file_id")?;
        let reference = ContentProviderRef::from_database(
            &provider,
            &provider_project_id,
            provider_release_id.as_deref(),
            provider_file_id.as_deref(),
        )?;
        refs_by_entry
            .entry(entry_id.clone())
            .or_insert_with(Vec::new)
            .push(reference);
        if row.try_get::<i64, _>("is_origin")? != 0 {
            origin_by_entry
                .insert(entry_id, ContentProvider::from_str(&provider)?);
        }
    }

    let entries_by_file_id = entries
        .iter()
        .filter_map(|entry| {
            entry.file_id.as_ref().map(|file_id| (file_id, entry))
        })
        .collect::<HashMap<_, _>>();
    let members_by_entry_id = members
        .iter()
        .filter_map(|member| {
            member
                .content_entry_id
                .as_ref()
                .map(|entry_id| (entry_id, member))
        })
        .collect::<HashMap<_, _>>();
    let mut local_metadata_by_entry = HashMap::new();
    for file in files.iter().filter(|file| !file.missing) {
        let Some(entry) = entries_by_file_id.get(&file.id) else {
            continue;
        };
        let Some(json) = file.local_mod_data.as_ref() else {
            continue;
        };
        if let Ok(metadata) =
            serde_json::from_str::<crate::mod_metadata::LocalModMetadata>(json)
        {
            local_metadata_by_entry.insert(entry.id.clone(), metadata);
        }
    }
    let mut represented_members = HashSet::new();
    let mut items = Vec::new();
    let files_for_listing = files.clone();

    for file in files.into_iter().filter(|file| !file.missing) {
        let Some(project_type) = project_type_for_file(&file) else {
            continue;
        };
        let entry = entries_by_file_id.get(&file.id).copied();
        let member =
            entry.and_then(|entry| members_by_entry_id.get(&entry.id).copied());
        if let Some(member) = member {
            represented_members.insert(member.id.as_str());
        }
        let ownership_kind = entry
            .map_or(ContentOwnershipKind::LocalDiscovered, |entry| {
                entry.ownership_kind
            });
        let provider_refs = entry
            .and_then(|entry| refs_by_entry.get(&entry.id))
            .cloned()
            .unwrap_or_default();
        let origin_provider = entry
            .and_then(|entry| origin_by_entry.get(&entry.id))
            .copied();
        let local_mod = file.local_mod_data.as_ref().and_then(|json| {
            serde_json::from_str::<crate::mod_metadata::LocalModMetadata>(json)
                .ok()
        });
        let title = local_mod
            .as_ref()
            .and_then(|metadata| metadata.name.clone())
            .or_else(|| {
                std::path::Path::new(&file.file_name)
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| file.file_name.clone());
        let external = provider_refs.is_empty();
        let content = ContentItem {
            file_name: file.file_name.clone(),
            file_path: file.relative_path.clone(),
            id: file.sha1.clone(),
            size: file.size,
            enabled: entry
                .map_or(file.enabled, |entry| entry.enabled && file.enabled),
            project_type,
            project: Some(ContentItemProject {
                id: local_mod
                    .as_ref()
                    .map(|metadata| format!("local:{}", metadata.mod_id))
                    .unwrap_or_else(|| format!("local:file:{}", file.sha1)),
                slug: local_mod
                    .as_ref()
                    .map(|metadata| metadata.mod_id.clone()),
                title,
                icon_url: file.icon_path.clone(),
                license: None,
            }),
            version: local_mod.as_ref().and_then(|metadata| {
                metadata.version.clone().map(|version| ContentItemVersion {
                    id: format!("local:{}", metadata.mod_id),
                    version_number: version,
                    file_name: file.file_name.clone(),
                    date_published: None,
                })
            }),
            owner: None,
            update: None,
            date_added: Some(file.modified_at.to_rfc3339()),
            provider_refs,
            origin_provider,
            rollback: None,
            environment: None,
            source_kind: entry.map(|entry| entry.source_kind),
            external,
            loader: local_mod.as_ref().and_then(|meta| meta.loader.clone()),
        };
        items.push(snapshot_item(
            Some(file.id.clone()),
            entry.map(|entry| entry.id.clone()),
            member,
            ownership_kind,
            project_type,
            file.relative_path,
            Some(content),
        ));
    }

    for member in &members {
        if represented_members.contains(member.id.as_str()) {
            continue;
        }
        if !filesystem::is_scannable_project_path(
            member.project_type,
            &member.expected_relative_path,
        ) {
            continue;
        }
        items.push(snapshot_item(
            None,
            member.content_entry_id.clone(),
            Some(member),
            ContentOwnershipKind::PackManaged,
            member.project_type,
            member.expected_relative_path.clone(),
            None,
        ));
    }

    // Dependency discovery can fetch metadata for every member of a pack.
    // Even an explicit refresh returns the file snapshot first; the slower
    // metadata pass publishes one lightweight follow-up snapshot when done.
    if refresh_remote {
        let instance_id = instance_id.to_string();
        let entries = entries.clone();
        let refs_by_entry = refs_by_entry.clone();
        let backfill_version = BackfillVersion {
            content_set_id: content_set.id.clone(),
            revision: content_set.revision,
        };
        let content_set = content_set.clone();
        let local_metadata_by_entry = local_metadata_by_entry.clone();
        let task_key = instance_id.clone();
        let guard_key = task_key.clone();
        // The generation is only known once this task is registered, which
        // happens after the spawn below; the guard receives it through a
        // oneshot so a replaced task never retires a newer registration.
        let (generation_sender, generation_receiver) =
            tokio::sync::oneshot::channel::<u64>();
        let handle = tokio::spawn(async move {
            let generation = generation_receiver.await.unwrap_or(u64::MAX);
            let _guard = BackfillGuard(guard_key, generation);
            let state = match State::get().await {
                Ok(state) => state,
                Err(error) => {
                    tracing::warn!(
                        "Content dependency backfill skipped: {error}"
                    );
                    return;
                }
            };
            let backfill = match backfill_dependency_edges_from_metadata(
                &entries,
                &refs_by_entry,
                &content_set,
                &local_metadata_by_entry,
                &state,
            )
            .await
            {
                Ok(backfill) => backfill,
                Err(error) => {
                    tracing::warn!(
                        "Content dependency backfill failed: {error}"
                    );
                    return;
                }
            };
            if backfill.is_empty() {
                return;
            }

            let _instance_lock = match state
                .lock_instance_content_with_timeout(
                    &instance_id,
                    CONTENT_LOCK_TIMEOUT,
                )
                .await
            {
                Ok(guard) => guard,
                Err(error) => {
                    tracing::warn!(
                        "Content dependency backfill could not lock the instance content: {error}"
                    );
                    return;
                }
            };
            let mut tx = match state.pool.begin_with("BEGIN IMMEDIATE").await {
                Ok(tx) => tx,
                Err(error) => {
                    tracing::warn!(
                        "Content dependency revision update failed: {error}"
                    );
                    return;
                }
            };
            let current: Option<(String, i64)> = match sqlx::query_as("SELECT content.id, content.revision FROM instances instance JOIN instance_content_sets content ON content.id = instance.applied_content_set_id WHERE instance.id = ?")
                .bind(&instance_id).fetch_optional(&mut *tx).await {
                Ok(current) => current,
                Err(error) => {
                    tracing::warn!(%error, "Content dependency backfill snapshot check failed");
                    return;
                }
            };
            if !current.is_some_and(|(id, revision)| {
                id == content_set.id
                    && u64::try_from(revision).ok()
                        == Some(content_set.revision)
            }) {
                return;
            }
            if let Err(error) = dependencies::persist_backfill(
                &backfill,
                &content_set.id,
                &mut tx,
            )
            .await
            {
                tracing::warn!(%error, "Content dependency reconciliation failed");
                return;
            }
            let revision =
                match content_rows::bump_content_set_revision_in_transaction(
                    &content_set.id,
                    &mut tx,
                )
                .await
                {
                    Ok(revision) => revision,
                    Err(error) => {
                        tracing::warn!(
                            "Content dependency revision update failed: {error}"
                        );
                        return;
                    }
                };
            if let Err(error) = tx.commit().await {
                tracing::warn!(
                    "Content dependency revision commit failed: {error}"
                );
                return;
            }
            if let Err(error) = crate::event::emit::emit_instance(
                &instance_id,
                crate::event::InstancePayloadType::ContentChanged { revision },
            )
            .await
            {
                tracing::warn!(
                    "Content dependency change event failed: {error}"
                );
            }
        });

        // Replace any backfill still running for this instance: its snapshot was
        // taken from an older content set, so letting both write would
        // interleave revisions.
        if let Some(generation) =
            replace_backfill_task(&task_key, backfill_version, handle).await
        {
            let _ = generation_sender.send(generation);
        }
    }

    attach_dependency_info(&mut items, &entries, &content_set.id, state)
        .await?;

    let cache_behaviour = if refresh_remote {
        CacheBehaviour::MustRevalidate
    } else {
        CacheBehaviour::CacheOnly
    };
    if enrich {
        match list_all_content_from_files(
            instance_id,
            files_for_listing,
            Some(cache_behaviour),
            refresh_remote,
            state,
        )
        .await
        {
            Ok(enriched) => {
                let enriched = enriched
                    .into_iter()
                    .map(|item| (item.file_path.clone(), item))
                    .collect::<HashMap<_, _>>();
                for item in &mut items {
                    if let Some(content) =
                        enriched.get(&item.expected_relative_path)
                    {
                        if refresh_remote
                            && item.member_id.is_none()
                            && let Some(reference) =
                                primary_persistent_ref(content)
                        {
                            item.provider = Some(reference.provider());
                            item.provider_project_id =
                                Some(reference.database_project_id());
                            item.provider_release_id =
                                reference.database_release_id();
                        }
                        item.content = Some(content.clone());
                        item.capabilities.can_update = content.update.is_some()
                            && item.materialization_state
                                == PackMemberMaterializationState::Present;
                    }
                }
            }
            Err(error) => warnings.push(InstanceContentWarning {
                code: "provider_metadata_unavailable".to_string(),
                message: error.to_string(),
                provider: None,
            }),
        }
    }

    let remote_pack = if enrich {
        match super::list_content::get_linked_modpack_info(
            instance_id,
            None,
            Some(cache_behaviour),
            state,
        )
        .await
        {
            Ok(pack) => pack,
            Err(error) => {
                warnings.push(InstanceContentWarning {
                    code: "pack_metadata_unavailable".to_string(),
                    message: error.to_string(),
                    provider: pack_provider(&link),
                });
                None
            }
        }
    } else {
        None
    };
    let mut pack = pack_snapshot(
        &instance.name,
        instance.icon_path.clone(),
        &link,
        &members,
        remote_pack,
    );

    let resource_store =
        crate::state::resources::ResourceStore::new(state.pool.clone());
    if let Some(pack) = &mut pack {
        pack.resource_id = resource_store
            .instance_pack_resource(instance_id)
            .await
            .map_err(|error| crate::ErrorKind::OtherError(error.to_string()))?;
    }
    let entry_resources = resource_store
        .entry_resources(instance_id)
        .await
        .map_err(|error| crate::ErrorKind::OtherError(error.to_string()))?;
    let pack_resources = resource_store
        .pack_resources(instance_id)
        .await
        .map_err(|error| crate::ErrorKind::OtherError(error.to_string()))?;
    for item in &mut items {
        item.resource_id = item
            .entry_id
            .as_ref()
            .and_then(|id| entry_resources.get(id))
            .cloned();
        item.pack_resource_id = item
            .member_id
            .as_ref()
            .and_then(|id| pack_resources.get(id))
            .cloned();
    }

    crate::resources::installed::attach_compatibility(
        &state.pool,
        instance_id,
        &content_set.game_version,
        content_set.loader.as_str(),
        &mut items,
        state.resources().await.ok(),
    )
    .await?;

    if enrich
        && let Err(error) = crate::resources::history::attach_to_snapshot(
            instance_id,
            &mut items,
            state,
        )
        .await
    {
        warnings.push(InstanceContentWarning {
            code: "resource_history_unavailable".into(),
            message: error.to_string(),
            provider: None,
        });
    }

    items.sort_by(|left, right| {
        left.expected_relative_path
            .to_lowercase()
            .cmp(&right.expected_relative_path.to_lowercase())
    });

    Ok(InstanceContentSnapshot {
        instance_id: instance_id.to_string(),
        revision: {
            let mut connection = state.pool.acquire().await?;
            content_rows::get_applied_content_set(instance_id, &mut *connection)
                .await?
        }
        .map_or(content_set.revision, |set| set.revision),
        pack,
        items,
        pending_manual_downloads,
        warnings,
    })
}

async fn attach_dependency_info(
    items: &mut [InstanceContentSnapshotItem],
    entries: &[crate::state::instances::ContentEntry],
    content_set_id: &str,
    state: &State,
) -> crate::Result<()> {
    let edges =
        content_rows::get_content_dependency_edges(content_set_id, &state.pool)
            .await?;
    let mut required_by: HashMap<String, Vec<ContentDependencyRef>> =
        HashMap::new();
    let mut requires: HashMap<String, Vec<ContentDependencyRef>> =
        HashMap::new();
    for edge in &edges {
        required_by
            .entry(edge.child_entry_id.clone())
            .or_default()
            .push(ContentDependencyRef {
                provider: edge.parent_provider,
                project_id: edge.parent_project_id.clone(),
                release_id: edge.parent_release_id.clone(),
            });
        requires
            .entry(edge.parent_entry_id.clone())
            .or_default()
            .push(ContentDependencyRef {
                provider: edge.child_provider,
                project_id: edge.child_project_id.clone(),
                release_id: edge.child_release_id.clone(),
            });
    }

    let entry_by_id = entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<HashMap<_, _>>();
    for item in items {
        let Some(entry_id) = item.entry_id.as_deref() else {
            continue;
        };
        let Some(entry) = entry_by_id.get(entry_id) else {
            continue;
        };
        let required_by = required_by.remove(entry_id).unwrap_or_default();
        let requires = requires.remove(entry_id).unwrap_or_default();
        if !entry.auto_dependency
            && required_by.is_empty()
            && requires.is_empty()
        {
            continue;
        }
        item.dependency = Some(ContentDependencyInfo {
            auto_dependency: entry.auto_dependency,
            orphaned: entry.auto_dependency && required_by.is_empty(),
            required_by,
            requires,
        });
    }
    Ok(())
}

fn snapshot_item(
    file_id: Option<String>,
    entry_id: Option<String>,
    member: Option<&crate::state::instances::PackMember>,
    ownership_kind: ContentOwnershipKind,
    project_type: crate::state::ProjectType,
    expected_relative_path: String,
    content: Option<ContentItem>,
) -> InstanceContentSnapshotItem {
    let persistent_ref = content.as_ref().and_then(primary_persistent_ref);
    let provider = member
        .and_then(|member| member.provider)
        .or_else(|| persistent_ref.map(ContentProviderRef::provider));
    let provider_project_id = member
        .and_then(|member| member.provider_project_id.clone())
        .or_else(|| {
            persistent_ref.map(ContentProviderRef::database_project_id)
        });
    let provider_release_id = member
        .and_then(|member| member.provider_release_id.clone())
        .or_else(|| {
            persistent_ref.and_then(ContentProviderRef::database_release_id)
        });
    let stored_materialization_state = member
        .map_or(PackMemberMaterializationState::Present, |member| {
            member.materialization_state
        });
    let materialization_state = snapshot_materialization_state(
        stored_materialization_state,
        content.is_some(),
    );
    let override_kind = member
        .map_or(PackMemberOverrideKind::None, |member| member.override_kind);
    let present = content.is_some()
        && materialization_state == PackMemberMaterializationState::Present;
    let has_provider = content
        .as_ref()
        .is_some_and(|content| !content.provider_refs.is_empty());

    InstanceContentSnapshotItem {
        artifact_ref: None,
        compatibility: None,
        resource_id: None,
        pack_resource_id: None,
        file_id,
        entry_id,
        member_id: member.map(|member| member.id.clone()),
        ownership_kind,
        materialization_state,
        override_kind,
        expected_relative_path,
        required: member.is_none_or(|member| member.required),
        project_type,
        provider,
        provider_project_id,
        provider_release_id,
        capabilities: ContentItemCapabilities {
            can_toggle: present,
            can_delete: present,
            can_update: present && has_provider,
            can_change_version: present && has_provider,
            can_restore_pack_default: member.is_some()
                && (override_kind != PackMemberOverrideKind::None
                    || materialization_state
                        == PackMemberMaterializationState::Missing),
        },
        content,
        dependency: None,
    }
}

fn primary_persistent_ref(
    content: &ContentItem,
) -> Option<&ContentProviderRef> {
    content
        .origin_provider
        .and_then(|origin| {
            content
                .provider_refs
                .iter()
                .find(|reference| reference.provider() == origin)
        })
        .or_else(|| content.provider_refs.first())
}

fn snapshot_materialization_state(
    stored: PackMemberMaterializationState,
    content_is_present: bool,
) -> PackMemberMaterializationState {
    if content_is_present && stored == PackMemberMaterializationState::Missing {
        PackMemberMaterializationState::Present
    } else {
        stored
    }
}

fn pack_snapshot(
    instance_name: &str,
    icon_path: Option<String>,
    link: &InstanceLink,
    members: &[crate::state::instances::PackMember],
    metadata: Option<crate::state::LinkedModpackInfo>,
) -> Option<InstanceContentPack> {
    let (provider, project_id, version_id, fallback_name) = match link {
        InstanceLink::ModrinthModpack {
            project_id,
            version_id,
        }
        | InstanceLink::ServerProjectModpack {
            content_project_id: project_id,
            content_version_id: version_id,
            ..
        } => (
            Some(ContentProvider::Modrinth),
            Some(project_id.clone()),
            Some(version_id.clone()),
            instance_name.to_string(),
        ),
        InstanceLink::CurseForgeModpack {
            project_id,
            version_id,
        } => (
            Some(ContentProvider::CurseForge),
            Some(project_id.clone()),
            Some(version_id.clone()),
            instance_name.to_string(),
        ),
        InstanceLink::ImportedModpack { name, .. } => (
            None,
            None,
            None,
            name.clone().unwrap_or_else(|| instance_name.to_string()),
        ),
        _ if members.is_empty() => return None,
        _ => (None, None, None, instance_name.to_string()),
    };
    let reconciled = members.iter().all(|member| member.reconciled);
    let name = metadata
        .as_ref()
        .map(|metadata| metadata.project.title.clone())
        .unwrap_or(fallback_name);

    Some(InstanceContentPack {
        resource_id: None,
        name,
        icon_path,
        provider,
        project_id,
        version_id,
        reconciled,
        can_update: provider.is_some() && reconciled,
        metadata,
    })
}

fn pack_provider(link: &InstanceLink) -> Option<ContentProvider> {
    match link {
        InstanceLink::CurseForgeModpack { .. } => {
            Some(ContentProvider::CurseForge)
        }
        InstanceLink::ModrinthModpack { .. }
        | InstanceLink::ServerProjectModpack { .. } => {
            Some(ContentProvider::Modrinth)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content_with_refs(
        provider_refs: Vec<ContentProviderRef>,
        origin_provider: Option<ContentProvider>,
    ) -> ContentItem {
        ContentItem {
            file_name: "example.jar".to_string(),
            file_path: "mods/example.jar".to_string(),
            id: "hash".to_string(),
            size: 1,
            enabled: true,
            project_type: crate::state::ProjectType::Mod,
            project: None,
            version: None,
            owner: None,
            update: None,
            date_added: None,
            provider_refs,
            origin_provider,
            rollback: None,
            environment: None,
            source_kind: Some(ContentSourceKind::Local),
            external: false,
            loader: None,
        }
    }

    #[test]
    fn present_content_does_not_keep_a_stale_missing_state() {
        assert_eq!(
            snapshot_materialization_state(
                PackMemberMaterializationState::Missing,
                true,
            ),
            PackMemberMaterializationState::Present,
        );
        assert_eq!(
            snapshot_materialization_state(
                PackMemberMaterializationState::Missing,
                false,
            ),
            PackMemberMaterializationState::Missing,
        );
    }

    #[test]
    fn verified_non_origin_ref_supplies_snapshot_identity() {
        let item = snapshot_item(
            Some("file".to_string()),
            Some("entry".to_string()),
            None,
            ContentOwnershipKind::UserAdded,
            crate::state::ProjectType::Mod,
            "mods/example.jar".to_string(),
            Some(content_with_refs(
                vec![
                    ContentProviderRef::from_database(
                        "modrinth",
                        "5LTBDHXu",
                        Some("eKsF3BdO"),
                        None,
                    )
                    .unwrap(),
                ],
                None,
            )),
        );

        assert_eq!(item.provider, Some(ContentProvider::Modrinth));
        assert_eq!(item.provider_project_id.as_deref(), Some("5LTBDHXu"));
        assert_eq!(item.provider_release_id.as_deref(), Some("eKsF3BdO"));
    }

    #[test]
    fn origin_ref_remains_primary_when_verified_alias_is_added() {
        let item = snapshot_item(
            Some("file".to_string()),
            Some("entry".to_string()),
            None,
            ContentOwnershipKind::UserAdded,
            crate::state::ProjectType::Mod,
            "mods/example.jar".to_string(),
            Some(content_with_refs(
                vec![
                    ContentProviderRef::from_database(
                        "curseforge",
                        "123",
                        Some("456"),
                        None,
                    )
                    .unwrap(),
                    ContentProviderRef::from_database(
                        "modrinth",
                        "5LTBDHXu",
                        Some("eKsF3BdO"),
                        None,
                    )
                    .unwrap(),
                ],
                Some(ContentProvider::CurseForge),
            )),
        );

        assert_eq!(item.provider, Some(ContentProvider::CurseForge));
        assert_eq!(item.provider_project_id.as_deref(), Some("123"));
        assert_eq!(item.provider_release_id.as_deref(), Some("456"));
    }

    #[test]
    fn upgraded_origin_release_wins_over_stale_same_provider_ref() {
        let item = snapshot_item(
            Some("file".to_string()),
            Some("entry".to_string()),
            None,
            ContentOwnershipKind::UserAdded,
            crate::state::ProjectType::Mod,
            "mods/[sodium] sodium-old-name.jar".to_string(),
            Some(content_with_refs(
                vec![
                    ContentProviderRef::from_database(
                        "modrinth",
                        "AANobbMI",
                        Some("vf7UgZpC"),
                        None,
                    )
                    .unwrap(),
                    ContentProviderRef::from_database(
                        "modrinth",
                        "AANobbMI",
                        Some("7pwil2dy"),
                        None,
                    )
                    .unwrap(),
                ],
                Some(ContentProvider::Modrinth),
            )),
        );

        assert_eq!(item.provider, Some(ContentProvider::Modrinth));
        assert_eq!(item.provider_project_id.as_deref(), Some("AANobbMI"));
        assert_eq!(item.provider_release_id.as_deref(), Some("vf7UgZpC"));
    }

    #[tokio::test]
    async fn a_newer_backfill_cancels_the_one_that_is_still_running() {
        let instance = "backfill-replaced";
        let finished =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let older = {
            let finished = finished.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                finished.store(true, std::sync::atomic::Ordering::SeqCst);
            })
        };
        // Give the older task a chance to start before it is replaced.
        tokio::task::yield_now().await;
        let older_generation = replace_backfill_task(
            instance,
            BackfillVersion {
                content_set_id: "content-a".into(),
                revision: 1,
            },
            older,
        )
        .await
        .unwrap();

        let newer = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });
        let newer_generation = replace_backfill_task(
            instance,
            BackfillVersion {
                content_set_id: "content-b".into(),
                revision: 2,
            },
            newer,
        )
        .await
        .unwrap();

        assert!(
            BACKFILL_TASKS.lock().await.contains_key(instance),
            "the newest backfill must be registered"
        );
        assert!(
            !finished.load(std::sync::atomic::Ordering::SeqCst),
            "the replaced backfill must be cancelled before it writes"
        );
        // The cancelled task retires only its own registration; the newer one
        // must survive it.
        forget_backfill_task(instance, older_generation).await;
        assert!(
            BACKFILL_TASKS.lock().await.contains_key(instance),
            "a replaced task must not remove its successor's registration"
        );
        assert_ne!(older_generation, newer_generation);
        forget_backfill_task(instance, newer_generation).await;
        assert!(!BACKFILL_TASKS.lock().await.contains_key(instance));
    }

    #[tokio::test]
    async fn same_snapshot_backfill_keeps_the_existing_task() {
        let instance = "backfill-same-snapshot";
        let version = BackfillVersion {
            content_set_id: "content-a".into(),
            revision: 4,
        };
        let existing = tokio::spawn(std::future::pending::<()>());
        let generation =
            replace_backfill_task(instance, version.clone(), existing)
                .await
                .unwrap();

        let duplicate = tokio::spawn(std::future::pending::<()>());
        assert!(
            replace_backfill_task(instance, version, duplicate)
                .await
                .is_none(),
            "an active task for the same content revision must be reused"
        );
        assert_eq!(
            BACKFILL_TASKS
                .lock()
                .await
                .get(instance)
                .map(|task| task.generation),
            Some(generation)
        );

        let replacement = tokio::spawn(std::future::pending::<()>());
        let next_generation = replace_backfill_task(
            instance,
            BackfillVersion {
                content_set_id: "content-a".into(),
                revision: 5,
            },
            replacement,
        )
        .await
        .unwrap();
        assert_ne!(generation, next_generation);
        forget_backfill_task(instance, generation).await;
        assert!(BACKFILL_TASKS.lock().await.contains_key(instance));
        forget_backfill_task(instance, next_generation).await;
    }

    #[tokio::test]
    async fn backfills_replaced_in_quick_succession_keep_the_last_one_registered()
     {
        let instance = "backfill-chain";
        let mut generations = Vec::new();
        for letter in ["a", "b", "c"] {
            let version = BackfillVersion {
                content_set_id: format!("{instance}-{letter}"),
                revision: 1,
            };
            let task = tokio::spawn(async {
                // Each scan stays parked until the test aborts or retires it.
                std::future::pending::<()>().await;
            });
            let generation = replace_backfill_task(instance, version, task)
                .await
                .unwrap();
            generations.push((letter, generation));
        }
        assert_ne!(generations[0].1, generations[1].1);
        assert_ne!(generations[1].1, generations[2].1);
        // The two cancelled scans try to clean up after being replaced; only the
        // last registration may remain.
        forget_backfill_task(instance, generations[0].1).await;
        forget_backfill_task(instance, generations[1].1).await;
        let registry = BACKFILL_TASKS.lock().await;
        assert!(
            registry.contains_key(instance),
            "only the last registration may survive the chain"
        );
        assert_eq!(
            registry.len(),
            1,
            "one instance keeps exactly one running backfill"
        );
        drop(registry);
        forget_backfill_task(instance, generations[2].1).await;
        assert!(!BACKFILL_TASKS.lock().await.contains_key(instance));
    }

    #[tokio::test]
    async fn a_finished_backfill_forgets_its_registry_entry() {
        let instance = "backfill-finished";
        let key = instance.to_string();
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let (generation_sender, generation_receiver) =
            tokio::sync::oneshot::channel::<u64>();
        let task = tokio::spawn(async move {
            let generation = generation_receiver.await.unwrap_or(u64::MAX);
            let _guard = BackfillGuard(key, generation);
            let _ = wait.await;
        });
        // Register first, then let the task end, so the cleanup cannot race the
        // registry write that this test performs.
        let generation = replace_backfill_task(
            instance,
            BackfillVersion {
                content_set_id: "content-a".into(),
                revision: 1,
            },
            task,
        )
        .await
        .unwrap();
        generation_sender.send(generation).unwrap();
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if !BACKFILL_TASKS.lock().await.contains_key(instance) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("a finished backfill must not stay registered");
    }
}
