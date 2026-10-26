use content_management::resources::{
    InstallScope, ReleaseSelection, ResourceOperationIntent, ResourceRef,
    ResourceSearchQuery, ResourceSearchSort, ResourceSelection, ResourceTarget,
    ResourceType, SelectionId, SourceId, SourceProjectId, SourceProjectRef,
    SourceReleaseId, SourceReleaseRef,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone, Serialize)]
pub struct LogFile {
    pub id: String,
    pub name: String,
    pub source_type: String,
    pub first_line: usize,
    pub line_count: usize,
    pub truncated: bool,
    #[serde(skip)]
    pub lines: Vec<String>,
}

#[derive(Deserialize, Serialize)]
pub struct ToolRequest {
    pub name: String,
    #[serde(default)]
    pub file_id: String,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub query: Option<String>,
    pub context: Option<usize>,
    pub instance_id: Option<String>,
    pub instance_name: Option<String>,
    pub game_version: Option<String>,
    pub loader: Option<String>,
    pub loader_version: Option<String>,
    pub patch: Option<Value>,
    pub java_version: Option<u32>,
    pub vendor: Option<String>,
    pub source: Option<String>,
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub resource_type: Option<String>,
    pub job_id: Option<String>,
    pub plan_id: Option<String>,
    pub plan_revision: Option<u32>,
    pub content_id: Option<String>,
    pub enabled: Option<bool>,
    pub install_dependencies: Option<bool>,
}

pub fn execute(files: &[LogFile], request: &ToolRequest) -> Value {
    if request.name == "list_log_files" {
        return json!(files);
    }
    let Some(file) = files.iter().find(|file| file.id == request.file_id)
    else {
        return json!({ "error": "Unknown file_id. Use list_log_files; filesystem paths are not accepted." });
    };
    let mut truncated;
    let selected: BTreeSet<usize> = match request.name.as_str() {
        "read_log_file" => {
            let start = request
                .start_line
                .unwrap_or(file.first_line)
                .max(file.first_line);
            let requested_end =
                request.end_line.unwrap_or(start.saturating_add(79));
            let end = requested_end.min(start.saturating_add(159));
            truncated = requested_end > end;
            (start.saturating_sub(file.first_line)
                ..=end.saturating_sub(file.first_line))
                .take(160)
                .filter(|index| *index < file.lines.len())
                .collect()
        }
        "grep_log_file" => {
            let query = request.query.as_deref().unwrap_or_default();
            if query.is_empty() || query.len() > 1024 {
                return json!({ "error": "Supply a nonempty literal query of at most 256 characters." });
            }
            let query = query.to_lowercase();
            let context = request.context.unwrap_or(3).min(10);
            let mut matches = file
                .lines
                .iter()
                .enumerate()
                .filter(|(_, line)| line.to_lowercase().contains(&query));
            let selected = matches
                .by_ref()
                .take(30)
                .flat_map(|(index, _)| {
                    index.saturating_sub(context)
                        ..=index
                            .saturating_add(context)
                            .min(file.lines.len().saturating_sub(1))
                })
                .collect();
            truncated = matches.next().is_some();
            selected
        }
        _ => return json!({ "error": "Unsupported diagnostic tool" }),
    };
    let mut remaining = 12_000usize;
    let mut lines = Vec::new();
    for index in selected {
        let text = &file.lines[index];
        let excerpt =
            text.chars().take(remaining.min(2000)).collect::<String>();
        remaining = remaining.saturating_sub(excerpt.chars().count());
        truncated |= excerpt.len() < text.len();
        lines.push(json!({ "line": file.first_line + index, "text": excerpt }));
        if remaining == 0 || lines.len() >= 160 {
            truncated = true;
            break;
        }
    }
    json!({ "file_id": file.id, "name": file.name, "first_line": file.first_line, "lines": lines, "truncated": truncated })
}

pub async fn execute_assistant(
    request: &ToolRequest,
    cancel: &CancellationToken,
) -> crate::Result<Value> {
    match request.name.as_str() {
        "get_instance_overview" => {
            let instances = crate::api::instance::list().await?;
            let total = instances.len();
            let items = instances
                .into_iter()
                .take(100)
                .map(|metadata| {
                    json!({
                        "id": metadata.instance.id,
                        "name": metadata.instance.name,
                        "game_version": metadata.applied_content_set.game_version,
                        "loader": metadata.applied_content_set.loader,
                        "loader_version": metadata.applied_content_set.loader_version,
                        "install_stage": metadata.instance.install_stage,
                    })
                })
                .collect::<Vec<_>>();
            Ok(
                json!({ "instances": items, "total_count": total, "truncated": total > 100 }),
            )
        }
        "get_launcher_settings" => {
            let state = crate::State::get().await?;
            let settings = crate::state::Settings::get(&state.pool).await?;
            Ok(json!({
                "locale": settings.locale,
                "theme": settings.theme,
                "accent_color": settings.accent_color,
                "collapsed_navigation": settings.collapsed_navigation,
                "toggle_sidebar": settings.toggle_sidebar,
                "memory": {
                    "automatic": settings.memory.automatic,
                    "maximum_megabytes": settings.memory.maximum,
                },
                "downloads": {
                    "automatic_concurrency": settings.auto_concurrent_downloads,
                    "maximum_concurrent_downloads": settings.max_concurrent_downloads,
                    "maximum_concurrent_writes": settings.max_concurrent_writes,
                    "minecraft_metadata_source": settings.minecraft_metadata_source,
                    "minecraft_file_source": settings.minecraft_file_source,
                    "modrinth_source": settings.modrinth_source,
                    "curseforge_source": settings.curseforge_source,
                },
            }))
        }
        "update_launcher_settings" => {
            let mut patch = request.patch.clone().ok_or_else(|| {
                input_error("update_launcher_settings requires a patch object")
            })?;
            normalize_settings_patch(&mut patch)?;
            validate_settings_patch(&patch)?;
            let settings = crate::api::settings::patch(patch).await?;
            Ok(launcher_settings(&settings))
        }
        "create_instance" => {
            let name = required_text(
                request.instance_name.as_deref(),
                "create_instance requires a name",
                100,
            )?;
            let game_version = required_text(
                request.game_version.as_deref(),
                "create_instance requires a game_version",
                64,
            )?;
            let loader = crate::state::ModLoader::try_from_string(
                request.loader.as_deref().unwrap_or("vanilla"),
            )?;
            let loader_version = request
                .loader_version
                .as_deref()
                .map(|value| bounded_text(value, "loader_version", 64))
                .transpose()?;
            let metadata = crate::api::instance::create(
                name,
                game_version,
                loader,
                loader_version,
                None,
                crate::state::InstanceLink::Unmanaged,
                None,
                None,
            )
            .await?;
            Ok(instance_summary(&metadata))
        }
        "rename_instance" => {
            let id = required_text(
                request.instance_id.as_deref(),
                "rename_instance requires an instance_id",
                128,
            )?;
            let name = required_text(
                request.instance_name.as_deref(),
                "rename_instance requires a name",
                100,
            )?;
            let metadata = crate::api::instance::edit(
                &id,
                crate::state::EditInstance {
                    name: Some(name),
                    ..Default::default()
                },
            )
            .await?;
            Ok(instance_summary(&metadata))
        }
        "delete_instance" => {
            let id = required_text(
                request.instance_id.as_deref(),
                "delete_instance requires an instance_id",
                128,
            )?;
            let instance = crate::api::instance::list()
                .await?
                .into_iter()
                .find(|metadata| metadata.instance.id.as_str() == id.as_str())
                .ok_or_else(|| input_error("Unknown instance"))?;
            if instance.instance.is_direct_linked() {
                return Err(input_error(
                    "Directly linked external instances cannot be deleted by the assistant",
                ));
            }
            crate::api::instance::remove(&id).await?;
            Ok(json!({ "deleted": true, "instance_id": id }))
        }
        "get_platform_diagnostics" => {
            let system = sysinfo::System::new_with_specifics(
                sysinfo::RefreshKind::nothing().with_memory(
                    sysinfo::MemoryRefreshKind::nothing().with_ram(),
                ),
            );
            let logical_processors = std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .ok();
            Ok(json!({
                "operating_system": std::env::consts::OS,
                "architecture": std::env::consts::ARCH,
                "logical_processors": logical_processors,
                "memory_bytes": {
                    "total": system.total_memory(),
                    "available": system.available_memory(),
                },
                "launcher_version": env!("CARGO_PKG_VERSION"),
            }))
        }
        "discover_java_installations" => {
            let installations =
                crate::api::jre::find_filtered_jres(None, false, false, false)
                    .await?;
            let defaults = crate::api::jre::get_java_default_versions().await?;
            let default_versions = defaults
                .into_iter()
                .map(|java| java.parsed_version)
                .collect::<std::collections::HashSet<_>>();
            Ok(json!({
                "installations": installations.into_iter().map(|java| json!({
                    "major_version": java.parsed_version,
                    "version": java.version,
                    "architecture": java.architecture,
                    "distribution": java.distribution,
                    "is_default": default_versions.contains(&java.parsed_version),
                })).collect::<Vec<_>>(),
            }))
        }
        "list_java_vendors" => {
            let vendors = crate::api::jre::list_java_feed_vendors().await?;
            Ok(json!({ "vendors": vendors }))
        }
        "inspect_instance_content" => {
            let instance_id = required_id(
                request.instance_id.as_deref(),
                "inspect_instance_content requires an instance_id",
            )?;
            let snapshot =
                crate::api::instance::get_content_snapshot_for_display(
                    &instance_id,
                )
                .await?;
            Ok(instance_content_summary(snapshot))
        }
        "search_resources" => {
            let query = resource_search_query(request)?;
            let state = crate::State::get().await?;
            let service = state.resources().await?.clone();
            let request_id = content_management::resources::RequestId::new(
                format!("agent-{}", Uuid::new_v4()),
            )
            .map_err(|error| input_error(&error.to_string()))?;
            let page = service
                .search(query, None, request_id, cancel)
                .await
                .map_err(|error| input_error(&error.to_string()))?;
            let entries = page
                .entries
                .into_iter()
                .map(|entry| {
                    json!({
                        "resource_id": entry.entry.resource_id,
                        "identity_status": entry.entry.status,
                        "projects": entry.projects.into_iter().map(|project| json!({
                            "source": project.project.source,
                            "project_id": project.project.project_id,
                            "slug": project.slug,
                            "title": project.title,
                            "summary": project.summary,
                            "types": project.types,
                            "downloads": project.downloads,
                            "game_versions": project.game_versions,
                            "loaders": project.loaders,
                        })).collect::<Vec<_>>(),
                    })
                })
                .take(20)
                .collect::<Vec<_>>();
            Ok(json!({
                "entries": entries,
                "total": page.total,
                "has_more": page.has_more,
                "source_failures": page.source_failures,
            }))
        }
        "get_resource_releases" => {
            let source = parse_source(request.source.as_deref())?;
            let project_id = required_id(
                request.project_id.as_deref(),
                "get_resource_releases requires a project_id",
            )?;
            let reference = ResourceRef::Source {
                project: SourceProjectRef {
                    source,
                    project_id: SourceProjectId::new(project_id)
                        .map_err(|error| input_error(&error.to_string()))?,
                },
            };
            let state = crate::State::get().await?;
            let service = state.resources().await?.clone();
            let request_id = content_management::resources::RequestId::new(
                format!("agent-{}", Uuid::new_v4()),
            )
            .map_err(|error| input_error(&error.to_string()))?;
            let page = service
                .releases(
                    &reference,
                    resource_target(
                        request.game_version.as_deref(),
                        request.loader.as_deref(),
                    ),
                    None,
                    request_id,
                    None,
                    cancel,
                )
                .await
                .map_err(|error| input_error(&error.to_string()))?;
            let releases = page
                .entries
                .into_iter()
                .map(|release| {
                    json!({
                        "release_id": release.reference.release_id,
                        "project_id": release.reference.project.project_id,
                        "source": release.reference.project.source,
                        "version": release.version,
                        "channel": release.channel,
                        "published_at": release.published_at,
                        "compatibility": release.compatibility,
                        "artifacts": release.artifacts.into_iter().map(|artifact| json!({
                            "artifact_id": artifact.reference.artifact_id,
                            "file_name": artifact.file_name,
                            "role": artifact.role,
                            "available": artifact.available,
                            "size": artifact.integrity.size,
                            "hashes": artifact.integrity.hashes,
                        })).collect::<Vec<_>>(),
                    })
                })
                .take(20)
                .collect::<Vec<_>>();
            Ok(json!({
                "releases": releases,
                "total": page.total,
                "has_more": page.has_more,
                "source_failures": page.source_failures,
            }))
        }
        "prepare_resource_install" => {
            let instance_id = required_id(
                request.instance_id.as_deref(),
                "prepare_resource_install requires an instance_id",
            )?;
            let source = parse_source(request.source.as_deref())?;
            let project_id = required_id(
                request.project_id.as_deref(),
                "prepare_resource_install requires a project_id",
            )?;
            let resource_type =
                parse_resource_type(request.resource_type.as_deref())?;
            let project = SourceProjectRef {
                source,
                project_id: SourceProjectId::new(project_id)
                    .map_err(|error| input_error(&error.to_string()))?,
            };
            let release = request
                .version_id
                .as_deref()
                .map(|version_id| {
                    SourceReleaseId::new(version_id.to_string())
                        .map(|release_id| ReleaseSelection::Release {
                            reference: SourceReleaseRef {
                                project: project.clone(),
                                release_id,
                            },
                        })
                        .map_err(|error| input_error(&error.to_string()))
                })
                .transpose()?
                .unwrap_or(ReleaseSelection::Automatic);
            let selection = ResourceSelection {
                id: SelectionId::new(format!("agent-{}", Uuid::new_v4()))
                    .map_err(|error| input_error(&error.to_string()))?,
                resource: ResourceRef::Source {
                    project: project.clone(),
                },
                release,
                resource_type,
                scope: InstallScope::Instance,
                entry_id: None,
            };
            let plan = crate::resources::plans::prepare_with_progress(
                instance_id,
                ResourceOperationIntent::Install,
                vec![selection],
                request.install_dependencies.unwrap_or(true),
                cancel,
                None,
                true,
            )
            .await
            .map_err(|error| input_error(&error.to_string()))?;
            Ok(resource_plan_summary(&plan))
        }
        "queue_resource_install" => {
            let plan_id = required_id(
                request.plan_id.as_deref(),
                "queue_resource_install requires a plan_id",
            )?;
            let revision = request.plan_revision.ok_or_else(|| {
                input_error("queue_resource_install requires a plan_revision")
            })?;
            let plan_id = content_management::resources::PlanId::new(plan_id)
                .map_err(|error| input_error(&error.to_string()))?;
            let job = crate::resources::plans::queue(plan_id, revision)
                .await
                .map_err(|error| input_error(&error.to_string()))?;
            Ok(install_job_summary(&job))
        }
        "get_install_job" => {
            let job_id = required_id(
                request.job_id.as_deref(),
                "get_install_job requires a job_id",
            )?;
            let job_id = Uuid::parse_str(&job_id).map_err(|_| {
                input_error("job_id must be a valid install job ID")
            })?;
            let job = crate::install::get_job(job_id).await?;
            Ok(install_job_summary(&job))
        }
        "cancel_install_job" => {
            let job_id = required_id(
                request.job_id.as_deref(),
                "cancel_install_job requires a job_id",
            )?;
            let job_id = Uuid::parse_str(&job_id).map_err(|_| {
                input_error("job_id must be a valid install job ID")
            })?;
            let job = crate::install::cancel_job(job_id).await?;
            Ok(install_job_summary(&job))
        }
        "install_java_runtime" => {
            let major_version = request.java_version.ok_or_else(|| {
                input_error("install_java_runtime requires a java_version")
            })?;
            if !(8..=25).contains(&major_version) {
                return Err(input_error(
                    "java_version must be between 8 and 25",
                ));
            }
            let vendors = crate::api::jre::list_java_feed_vendors().await?;
            let vendor = required_text(
                request.vendor.as_deref(),
                "install_java_runtime requires a vendor from list_java_vendors",
                64,
            )?;
            if !vendors.iter().any(|available| available == &vendor) {
                return Err(input_error(&format!(
                    "Unsupported Java vendor. Available vendors: {}",
                    vendors.join(", ")
                )));
            }
            let job =
                crate::install::download_java(vendor, major_version).await?;
            Ok(install_job_summary(&job))
        }
        "set_java_default_runtime" => {
            let major_version = request.java_version.ok_or_else(|| {
                input_error("set_java_default_runtime requires a java_version")
            })?;
            let java = crate::api::jre::find_filtered_jres(
                Some(major_version),
                false,
                false,
                false,
            )
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                input_error("No installed Java runtime matches this version")
            })?;
            let selected = crate::api::jre::set_java_default_version(
                major_version,
                java.path,
            )
            .await?;
            Ok(json!({
                "major_version": selected.parsed_version,
                "version": selected.version,
                "architecture": selected.architecture,
                "default_updated": true,
            }))
        }
        "check_content_updates" => {
            let instance_id = required_id(
                request.instance_id.as_deref(),
                "check_content_updates requires an instance_id",
            )?;
            let updates = crate::resources::updates::check(
                instance_id,
                None,
                |_| {},
                cancel,
            )
            .await
            .map_err(|error| input_error(&error.to_string()))?;
            Ok(json!({
                "update_count": updates.entries.len(),
                "entries": updates.entries,
                "releases": updates.releases,
                "unresolved_count": updates.unresolved.len(),
                "failed_entry_count": updates.failed_entries.len(),
                "failures": updates.failures,
            }))
        }
        "prepare_resource_update" => {
            let instance_id = required_id(
                request.instance_id.as_deref(),
                "prepare_resource_update requires an instance_id",
            )?;
            let content_id = required_id(
                request.content_id.as_deref(),
                "prepare_resource_update requires a content_id",
            )?;
            let entry_id =
                content_management::resources::InstalledEntryId::new(
                    content_id,
                )
                .map_err(|error| input_error(&error.to_string()))?;
            let plan = crate::resources::installed::prepare_entries(
                instance_id,
                ResourceOperationIntent::Update,
                vec![entry_id],
                None,
                true,
                cancel,
            )
            .await
            .map_err(|error| input_error(&error.to_string()))?;
            Ok(resource_plan_summary(&plan))
        }
        "disable_content" | "enable_content" => {
            let instance_id = required_id(
                request.instance_id.as_deref(),
                "content operation requires an instance_id",
            )?;
            let content_id = required_id(
                request.content_id.as_deref(),
                "content operation requires a content_id",
            )?;
            let enabled = request.name == "enable_content";
            let result = crate::api::instance::toggle_content_entry(
                &instance_id,
                &content_id,
                Some(enabled),
            )
            .await?;
            let _ = result;
            Ok(
                json!({ "instance_id": instance_id, "content_id": content_id, "enabled": enabled }),
            )
        }
        "remove_content" => {
            let instance_id = required_id(
                request.instance_id.as_deref(),
                "remove_content requires an instance_id",
            )?;
            let content_id = required_id(
                request.content_id.as_deref(),
                "remove_content requires a content_id",
            )?;
            crate::api::instance::remove_content_entry(
                &instance_id,
                &content_id,
            )
            .await?;
            Ok(
                json!({ "instance_id": instance_id, "content_id": content_id, "removed": true }),
            )
        }
        _ => Err(crate::ErrorKind::InputError(
            "Unsupported launcher assistant tool".into(),
        )
        .into()),
    }
}

fn input_error(message: &str) -> crate::Error {
    crate::ErrorKind::InputError(message.to_string()).into()
}

fn required_id(value: Option<&str>, message: &str) -> crate::Result<String> {
    let value = value.ok_or_else(|| input_error(message))?;
    bounded_text(value, "id", 160)
}

fn parse_source(value: Option<&str>) -> crate::Result<SourceId> {
    match value.unwrap_or_default() {
        "modrinth" => Ok(SourceId::modrinth()),
        "curseforge" => Ok(SourceId::curseforge()),
        "mcarchive" => Ok(SourceId::mcarchive()),
        "planet_minecraft" => Ok(SourceId::planet_minecraft()),
        _ => Err(input_error(
            "source must be modrinth, curseforge, mcarchive, or planet_minecraft",
        )),
    }
}

fn parse_resource_type(value: Option<&str>) -> crate::Result<ResourceType> {
    match value.unwrap_or_default() {
        "mod" => Ok(ResourceType::Mod),
        "plugin" => Ok(ResourceType::Plugin),
        "data_pack" | "dataPack" => Ok(ResourceType::DataPack),
        "resource_pack" | "resourcePack" => Ok(ResourceType::ResourcePack),
        "shader" => Ok(ResourceType::Shader),
        "mod_pack" | "modPack" => Ok(ResourceType::ModPack),
        "world" => Ok(ResourceType::World),
        _ => Err(input_error(
            "resource_type must be mod, data_pack, resource_pack, shader, mod_pack, or world",
        )),
    }
}

fn resource_target(
    game_version: Option<&str>,
    loader: Option<&str>,
) -> ResourceTarget {
    ResourceTarget {
        game_versions: game_version
            .filter(|value| !value.trim().is_empty())
            .map(|value| vec![value.trim().to_string()])
            .unwrap_or_default(),
        loaders: loader
            .filter(|value| !value.trim().is_empty())
            .map(|value| vec![value.trim().to_string()])
            .unwrap_or_default(),
        channels: Vec::new(),
    }
}

fn resource_search_query(
    request: &ToolRequest,
) -> crate::Result<ResourceSearchQuery> {
    let text = bounded_text(
        request.query.as_deref().unwrap_or_default(),
        "query",
        200,
    )?;
    let types = request
        .resource_type
        .as_deref()
        .map(|value| parse_resource_type(Some(value)).map(|value| vec![value]))
        .transpose()?
        .unwrap_or_else(|| {
            vec![
                ResourceType::Mod,
                ResourceType::ResourcePack,
                ResourceType::Shader,
                ResourceType::DataPack,
                ResourceType::World,
            ]
        });
    let sources = request
        .source
        .as_deref()
        .map(|value| parse_source(Some(value)).map(|value| vec![value]))
        .transpose()?
        .unwrap_or_default();
    Ok(ResourceSearchQuery {
        filter: None,
        text,
        types,
        sources,
        target: resource_target(
            request.game_version.as_deref(),
            request.loader.as_deref(),
        ),
        categories: Vec::new(),
        sort: ResourceSearchSort::Relevance,
        page_size: request.context.unwrap_or(20).clamp(1, 20) as u32,
    })
}

fn instance_content_summary(
    snapshot: crate::state::InstanceContentSnapshot,
) -> Value {
    let item_count = snapshot.items.len();
    let items = snapshot
        .items
        .into_iter()
        .take(200)
        .map(|item| {
            let content = item.content;
            json!({
                "content_id": item.entry_id.or(item.member_id).or(item.file_id),
                "file_name": content.as_ref().map(|value| value.file_name.clone()),
                "project_id": item.provider_project_id,
                "project_title": content.as_ref().and_then(|value| value.project.as_ref()).map(|project| project.title.clone()),
                "version": content.as_ref().and_then(|value| value.version.as_ref()).map(|version| version.version_number.clone()),
                "project_type": item.project_type,
                "provider": item.provider,
                "enabled": content.as_ref().map(|value| value.enabled),
                "update_available": content.as_ref().is_some_and(|value| value.update.is_some()),
                "required": item.required,
                "relative_path": item.expected_relative_path,
                "compatibility": item.compatibility,
                "capabilities": item.capabilities,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "instance_id": snapshot.instance_id,
        "revision": snapshot.revision,
        "pack": snapshot.pack.map(|pack| json!({
            "name": pack.name,
            "provider": pack.provider,
            "project_id": pack.project_id,
            "version_id": pack.version_id,
            "reconciled": pack.reconciled,
            "can_update": pack.can_update,
        })),
        "items": items,
        "pending_manual_downloads": snapshot.pending_manual_downloads.len(),
        "warnings": snapshot.warnings,
        "truncated": item_count > 200,
    })
}

fn resource_plan_summary(
    plan: &content_management::resources::ResourceOperationPlan,
) -> Value {
    let blocking = plan.issues.iter().any(|issue| issue.blocking);
    let actions = plan
        .actions
        .iter()
        .take(100)
        .map(|action| {
            json!({
                "entry_id": action.entry_id,
                "kind": action.kind,
                "target_relative_path": action.target_relative_path,
                "file_name": action.target.file_name,
                "size": action.target.integrity.size,
                "source": action.target.reference.release.project.source,
                "project_id": action.target.reference.release.project.project_id,
                "release_id": action.target.reference.release.release_id,
            })
        })
        .collect::<Vec<_>>();
    let nodes = plan
        .nodes
        .iter()
        .take(100)
        .map(|node| {
            json!({
                "node_id": node.id,
                "resource": node.resource,
                "state": node.state,
                "resource_type": node.resource_type,
                "file_name": node.artifact.as_ref().and_then(|artifact| artifact.file_name.as_ref()),
                "size": node.artifact.as_ref().and_then(|artifact| artifact.integrity.size),
                "compatibility": node.compatibility,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "plan_id": plan.id,
        "revision": plan.revision,
        "instance_id": plan.instance_id,
        "instance_revision": plan.instance_revision,
        "intent": plan.intent,
        "action_count": plan.actions.len(),
        "actions": actions,
        "node_count": plan.nodes.len(),
        "nodes": nodes,
        "dependency_count": plan.edges.len(),
        "dependencies": plan.edges.iter().take(100).map(|edge| json!({
            "parent": edge.parent,
            "child": edge.child,
            "kind": edge.kind,
            "version_constraint": edge.version_constraint,
        })).collect::<Vec<_>>(),
        "issues": plan.issues,
        "blocking": blocking,
        "truncated": plan.actions.len() > 100,
    })
}

fn install_job_summary(snapshot: &crate::install::InstallJobSnapshot) -> Value {
    json!({
        "job_id": snapshot.job_id,
        "instance_id": snapshot.instance_id,
        "kind": snapshot.kind,
        "status": snapshot.status,
        "phase": snapshot.phase,
        "progress": snapshot.progress,
        "summary": {
            "files_completed": snapshot.summary.files_completed,
            "files_total": snapshot.summary.files_total,
            "bytes_downloaded": snapshot.summary.bytes_downloaded,
            "bytes_total": snapshot.summary.bytes_total,
            "speed_bytes_per_second": snapshot.summary.speed_bytes_per_second,
            "eta_seconds": snapshot.summary.eta_seconds,
            "fallback_count": snapshot.summary.fallback_count,
        },
        "error": snapshot.error.as_ref().map(|error| json!({
            "code": error.code,
            "phase": error.phase,
        })),
    })
}

fn bounded_text(value: &str, field: &str, max: usize) -> crate::Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > max
        || value.chars().any(char::is_control)
    {
        return Err(input_error(&format!(
            "{field} must be a nonempty safe string"
        )));
    }
    Ok(value.to_string())
}

fn required_text(
    value: Option<&str>,
    message: &str,
    max: usize,
) -> crate::Result<String> {
    let value = value.ok_or_else(|| input_error(message))?;
    bounded_text(value, "value", max)
}

fn instance_summary(metadata: &crate::state::InstanceMetadata) -> Value {
    json!({
        "id": metadata.instance.id,
        "name": metadata.instance.name,
        "game_version": metadata.applied_content_set.game_version,
        "loader": metadata.applied_content_set.loader,
        "loader_version": metadata.applied_content_set.loader_version,
        "install_stage": metadata.instance.install_stage,
    })
}

fn launcher_settings(settings: &crate::state::Settings) -> Value {
    json!({
        "locale": settings.locale,
        "theme": settings.theme,
        "accent_color": settings.accent_color,
        "collapsed_navigation": settings.collapsed_navigation,
        "toggle_sidebar": settings.toggle_sidebar,
        "memory": {
            "automatic": settings.memory.automatic,
            "maximum_megabytes": settings.memory.maximum,
        },
        "downloads": {
            "automatic_concurrency": settings.auto_concurrent_downloads,
            "maximum_concurrent_downloads": settings.max_concurrent_downloads,
            "maximum_concurrent_writes": settings.max_concurrent_writes,
            "minecraft_metadata_source": settings.minecraft_metadata_source,
            "minecraft_file_source": settings.minecraft_file_source,
            "modrinth_source": settings.modrinth_source,
            "curseforge_source": settings.curseforge_source,
        },
    })
}

fn normalize_settings_patch(patch: &mut Value) -> crate::Result<()> {
    let Some(object) = patch.as_object_mut() else {
        return Err(input_error("Settings patch must be an object"));
    };
    let Some(memory) = object.get_mut("memory").and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    if let Some(maximum) = memory.remove("maximum_megabytes") {
        if memory.contains_key("maximum") {
            return Err(input_error(
                "memory must not contain both maximum and maximum_megabytes",
            ));
        }
        memory.insert("maximum".to_string(), maximum);
    }
    Ok(())
}

fn validate_settings_patch(patch: &Value) -> crate::Result<()> {
    let Some(object) = patch.as_object() else {
        return Err(input_error("Settings patch must be an object"));
    };
    if object.is_empty() || object.len() > 8 {
        return Err(input_error("Settings patch must contain 1 to 8 fields"));
    }
    for (key, value) in object {
        let valid = match key.as_str() {
            "locale" => value.as_str().is_some_and(|text| {
                !text.trim().is_empty()
                    && text.len() <= 32
                    && !text.chars().any(char::is_control)
            }),
            "theme" => value.as_str().is_some_and(|value| {
                matches!(value, "dark" | "light" | "oled" | "system")
            }),
            "accent_color" => value.as_str().is_some_and(|value| {
                matches!(
                    value,
                    "pink" | "orange" | "green" | "blue" | "purple" | "system"
                ) || value.strip_prefix("custom:#").is_some_and(|hex| {
                    hex.len() == 6
                        && hex
                            .chars()
                            .all(|character| character.is_ascii_hexdigit())
                })
            }),
            "collapsed_navigation"
            | "toggle_sidebar"
            | "auto_concurrent_downloads" => value.is_boolean(),
            "max_concurrent_downloads" | "max_concurrent_writes" => value
                .as_u64()
                .is_some_and(|value| (1..=256).contains(&value)),
            "memory" => {
                let Some(memory) = value.as_object() else {
                    return Err(input_error("memory must be an object"));
                };
                !memory.is_empty()
                    && memory.keys().all(|key| {
                        matches!(
                            key.as_str(),
                            "automatic" | "maximum" | "optimize_before_launch"
                        )
                    })
                    && memory.iter().all(|(key, value)| match key.as_str() {
                        "automatic" | "optimize_before_launch" => {
                            value.is_boolean()
                        }
                        "maximum" => value.as_u64().is_some_and(|value| {
                            (256..=1_048_576).contains(&value)
                        }),
                        _ => false,
                    })
            }
            "minecraft_metadata_source"
            | "minecraft_file_source"
            | "modrinth_source"
            | "curseforge_source" => value.as_str().is_some_and(|value| {
                matches!(
                    value,
                    "auto"
                        | "official_only"
                        | "mirror_preferred"
                        | "official_preferred"
                )
            }),
            _ => false,
        };
        if !valid {
            return Err(input_error(&format!(
                "Unsupported or invalid setting: {key}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file() -> LogFile {
        LogFile {
            id: "log-1".into(),
            name: "latest.log".into(),
            source_type: "minecraft_log".into(),
            first_line: 200,
            line_count: 5,
            truncated: true,
            lines: ["start", "ERROR one", "cause", "error two", "end"]
                .map(str::to_string)
                .into(),
        }
    }

    #[test]
    fn grep_merges_context_and_preserves_original_lines() {
        let result = execute(
            &[file()],
            &ToolRequest {
                name: "grep_log_file".into(),
                file_id: "log-1".into(),
                start_line: None,
                end_line: None,
                query: Some("error".into()),
                context: Some(1),
                instance_id: None,
                instance_name: None,
                game_version: None,
                loader: None,
                loader_version: None,
                patch: None,
                java_version: None,
                vendor: None,
                source: None,
                project_id: None,
                version_id: None,
                resource_type: None,
                job_id: None,
                plan_id: None,
                plan_revision: None,
                content_id: None,
                enabled: None,
                install_dependencies: None,
            },
        );
        let lines = result["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0]["line"], 200);
        assert_eq!(lines[4]["line"], 204);
    }

    #[test]
    fn rejects_paths_and_bounds_long_lines() {
        let mut request = ToolRequest {
            name: "read_log_file".into(),
            file_id: "../../secret".into(),
            start_line: Some(200),
            end_line: Some(usize::MAX),
            query: None,
            context: None,
            instance_id: None,
            instance_name: None,
            game_version: None,
            loader: None,
            loader_version: None,
            patch: None,
            java_version: None,
            vendor: None,
            source: None,
            project_id: None,
            version_id: None,
            resource_type: None,
            job_id: None,
            plan_id: None,
            plan_revision: None,
            content_id: None,
            enabled: None,
            install_dependencies: None,
        };
        assert!(execute(&[file()], &request).get("error").is_some());
        request.file_id = "log-1".into();
        let mut file = file();
        file.lines = vec!["字".repeat(20000)];
        let result = execute(&[file], &request);
        assert_eq!(
            result["lines"][0]["text"].as_str().unwrap().chars().count(),
            2000
        );
        assert_eq!(result["truncated"], true);
        request.start_line = Some(usize::MAX);
        assert_eq!(
            execute(&[self::file()], &request)["lines"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }
}
