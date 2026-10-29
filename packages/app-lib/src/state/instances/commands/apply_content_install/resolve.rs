use super::*;

use crate::state::{Dependency, DependencyType, Version};
use async_trait::async_trait;
use content_management::{ContentMetadataProvider, Error as ResolveError};

struct CachedEntryContentProvider {
    cache_behaviour: Option<CacheBehaviour>,
}

#[async_trait]
impl ContentMetadataProvider for CachedEntryContentProvider {
    async fn get_version(
        &mut self,
        version_id: String,
    ) -> Result<Option<content_management::Version>, ResolveError> {
        let values = CachedEntry::get_resource_values(
            CacheValueType::Version,
            vec![version_id],
            self.cache_behaviour,
        )
        .await
        .map_err(resolve_provider_error)?;
        let version = values.into_iter().find_map(|value| match value {
            CacheValue::Version(version) => Some(version),
            _ => None,
        });

        Ok(version.map(version_to_resolver))
    }

    async fn get_project_versions(
        &mut self,
        project_id: String,
    ) -> Result<Vec<content_management::Version>, ResolveError> {
        let values = CachedEntry::get_resource_values(
            CacheValueType::ProjectVersions,
            vec![project_id],
            self.cache_behaviour,
        )
        .await
        .map_err(resolve_provider_error)?;
        let versions = values.into_iter().find_map(|value| match value {
            CacheValue::ProjectVersions(versions) => Some(versions.versions),
            _ => None,
        });

        Ok(versions
            .unwrap_or_default()
            .into_iter()
            .map(version_to_resolver)
            .collect())
    }
}

fn resolve_provider_error(error: crate::Error) -> ResolveError {
    ResolveError::Provider(error.to_string())
}

pub(super) async fn cached_modrinth_version(
    version_id: ModrinthVersionId,
    cache_behaviour: Option<CacheBehaviour>,
) -> crate::Result<Option<Version>> {
    let values = CachedEntry::get_resource_values(
        CacheValueType::Version,
        vec![version_id.to_string()],
        cache_behaviour,
    )
    .await?;
    Ok(values.into_iter().find_map(|value| match value {
        CacheValue::Version(version) => Some(version),
        _ => None,
    }))
}

async fn cached_modrinth_projects(
    project_ids: Vec<ModrinthProjectId>,
    cache_behaviour: Option<CacheBehaviour>,
) -> crate::Result<Vec<Project>> {
    let project_ids = project_ids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let values = CachedEntry::get_resource_values(
        CacheValueType::Project,
        project_ids,
        cache_behaviour,
    )
    .await?;
    Ok(values
        .into_iter()
        .filter_map(|value| match value {
            CacheValue::Project(project) => Some(project),
            _ => None,
        })
        .collect())
}

pub(super) async fn cached_modrinth_project(
    project_id: ModrinthProjectId,
    cache_behaviour: Option<CacheBehaviour>,
) -> crate::Result<Option<Project>> {
    let values = CachedEntry::get_resource_values(
        CacheValueType::Project,
        vec![project_id.to_string()],
        cache_behaviour,
    )
    .await?;
    Ok(values.into_iter().find_map(|value| match value {
        CacheValue::Project(project) => Some(project),
        _ => None,
    }))
}

fn resolver_error(error: ResolveError) -> crate::Error {
    crate::ErrorKind::InputError(error.to_string()).into()
}

fn version_to_resolver(version: Version) -> content_management::Version {
    let (filename, sha1) = version
        .files
        .first()
        .map(|f| (Some(f.filename.clone()), f.hashes.get("sha1").cloned()))
        .unwrap_or_default();

    content_management::Version {
        id: version.id,
        project_id: version.project_id,
        date_published: version.date_published,
        version_type: version.version_type,
        dependencies: version
            .dependencies
            .into_iter()
            .map(dependency_to_resolver)
            .collect(),
        game_versions: version.game_versions,
        loaders: version.loaders,
        version_number: Some(version.version_number),
        filename,
        sha1,
    }
}

fn dependency_to_resolver(
    dependency: Dependency,
) -> content_management::Dependency {
    content_management::Dependency {
        version_id: dependency.version_id,
        project_id: dependency.project_id,
        file_name: dependency.file_name,
        dependency_type: match dependency.dependency_type {
            DependencyType::Required => {
                content_management::DependencyType::Required
            }
            DependencyType::Optional => {
                content_management::DependencyType::Optional
            }
            DependencyType::Incompatible => {
                content_management::DependencyType::Incompatible
            }
            DependencyType::Embedded => {
                content_management::DependencyType::Embedded
            }
        },
    }
}

pub(super) fn target_preferences(
    game_version: String,
    loader: ModLoader,
    content_type: ContentType,
) -> ResolutionPreferences {
    let loader = match content_type {
        ContentType::DataPack => "datapack".to_string(),
        ContentType::ResourcePack => "minecraft".to_string(),
        ContentType::Shader => "iris".to_string(),
        _ => loader.as_str().to_string(),
    };

    ResolutionPreferences {
        game_versions: if content_type == ContentType::ResourcePack {
            Vec::new()
        } else {
            vec![game_version]
        },
        loaders: vec![loader],
    }
}

pub(crate) async fn resolve_install_plan(
    instance_id: &str,
    request: InstanceInstallProjectRequest,
    state: &State,
) -> crate::Result<ResolveContentPlan> {
    resolve_install_plan_with_cache(
        instance_id.to_string(),
        request,
        Some(CacheBehaviour::MustRevalidate),
        DependencyPolicy::IncludeOptional,
        state.pool.clone(),
    )
    .await
}

pub(crate) async fn resolve_update_plan(
    instance_id: &str,
    request: InstanceInstallProjectRequest,
    state: &State,
) -> crate::Result<ResolveContentPlan> {
    resolve_install_plan_with_cache(
        instance_id.to_string(),
        request,
        Some(CacheBehaviour::MustRevalidate),
        DependencyPolicy::RequiredOnly,
        state.pool.clone(),
    )
    .await
}

pub(super) async fn resolve_install_plan_with_cache(
    instance_id: String,
    request: InstanceInstallProjectRequest,
    cache_behaviour: Option<CacheBehaviour>,
    dependency_policy: DependencyPolicy,
    pool: sqlx::SqlitePool,
) -> crate::Result<ResolveContentPlan> {
    let content_set = {
        let mut connection = pool.acquire().await?;
        content_rows::get_applied_content_set(&instance_id, &mut *connection)
            .await?
    }
    .ok_or_else(|| {
        crate::ErrorKind::InputError(format!(
            "Instance {instance_id} has no applied content set"
        ))
    })?;
    let existing_project_ids =
        crate::state::instances::commands::get_installed_project_ids_for_instance_with_pool(
            instance_id.clone(),
            None,
            pool.clone(),
        )
        .await?;
    let provider = CachedEntryContentProvider { cache_behaviour };
    let content_type = request.content_type;
    let request = ResolveContentRequest {
        project_id: request.project_id,
        version_id: request.version_id,
        content_type,
        selected: request.selected,
        target: target_preferences(
            content_set.game_version,
            content_set.loader,
            content_type,
        ),
        existing_project_ids,
        excluded_project_ids: request.excluded_project_ids,
        force_project_ids: request.force_project_ids,
    };

    let mut plan = content_management::resolve_content_with_policy(
        provider,
        request,
        dependency_policy,
    )
    .await
    .map_err(resolver_error)?;

    // Enrich the resolved graph with project-level metadata from the cache.
    let project_ids: Vec<ModrinthProjectId> =
        std::iter::once(&plan.primary.project_id)
            .chain(
                plan.dependencies
                    .iter()
                    .map(|dependency| &dependency.project_id),
            )
            .filter_map(|id| ModrinthProjectId::new(id.clone()).ok())
            .collect();

    if !project_ids.is_empty() {
        let projects =
            cached_modrinth_projects(project_ids, cache_behaviour).await?;
        let project_map: HashMap<String, &Project> = projects
            .iter()
            .map(|project| (project.id.clone(), project))
            .collect();

        let enrich = |content: &mut ResolvedContent| {
            let metadata =
                content.metadata.get_or_insert_with(Default::default);
            if let Some(project) = project_map.get(&content.project_id) {
                metadata.title = Some(project.title.clone());
                metadata.icon_url = project.icon_url.clone();
            }
        };

        enrich(&mut plan.primary);
        for dependency in &mut plan.dependencies {
            enrich(dependency);
        }
    }

    Ok(plan)
}

pub(crate) async fn resolve_install_plan_for_target_with_policy(
    request: InstanceInstallProjectRequest,
    game_version: String,
    loader: ModLoader,
    dependency_policy: DependencyPolicy,
) -> crate::Result<ResolveContentPlan> {
    let cache_behaviour = Some(CacheBehaviour::MustRevalidate);
    let provider = CachedEntryContentProvider { cache_behaviour };
    let content_type = request.content_type;
    let request = ResolveContentRequest {
        project_id: request.project_id,
        version_id: request.version_id,
        content_type,
        selected: request.selected,
        target: target_preferences(game_version, loader, content_type),
        existing_project_ids: Vec::new(),
        excluded_project_ids: request.excluded_project_ids,
        force_project_ids: request.force_project_ids,
    };

    let mut plan = content_management::resolve_content_with_policy(
        provider,
        request,
        dependency_policy,
    )
    .await
    .map_err(resolver_error)?;

    // Enrich the resolved graph with project-level metadata from the cache.
    let project_ids: Vec<ModrinthProjectId> =
        std::iter::once(&plan.primary.project_id)
            .chain(
                plan.dependencies
                    .iter()
                    .map(|dependency| &dependency.project_id),
            )
            .filter_map(|id| ModrinthProjectId::new(id.clone()).ok())
            .collect();

    if !project_ids.is_empty() {
        let projects =
            cached_modrinth_projects(project_ids, cache_behaviour).await?;
        let project_map: HashMap<String, &Project> = projects
            .iter()
            .map(|project| (project.id.clone(), project))
            .collect();

        let enrich = |content: &mut ResolvedContent| {
            let metadata =
                content.metadata.get_or_insert_with(Default::default);
            if let Some(project) = project_map.get(&content.project_id) {
                metadata.title = Some(project.title.clone());
                metadata.icon_url = project.icon_url.clone();
            }
        };

        enrich(&mut plan.primary);
        for dependency in &mut plan.dependencies {
            enrich(dependency);
        }
    }

    Ok(plan)
}
