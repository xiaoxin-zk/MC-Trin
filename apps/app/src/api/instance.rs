use crate::api::Result;
use crate::api::files::local_instance_icon_path;
use chrono::NaiveDate;
use dashmap::DashMap;
use futures::{StreamExt, TryStreamExt};
use mctrin::data::{
    AppliedContentSetPatch, ContentItem, Dependency,
    EditInstance as CoreEditInstance, InstanceInstallCandidate,
    InstanceInstallTarget, InstanceLaunchOverridesPatch,
    InstanceLink as CoreInstanceLink, InstanceMetadata, LinkedModpackInfo,
};
use mctrin::instance::InstallContentBatchRequest;
use mctrin::instance::InstanceGroup;
use mctrin::instance::InstanceGroupMembershipChange;
use mctrin::instance::InstanceGroupMembershipUpdate;
use mctrin::instance::QuickPlayType;
use mctrin::pack::import::ImportLauncherType;
use mctrin::prelude::*;
use mctrin::server_address::ServerAddress;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_fs::FsExt;
use tauri_plugin_opener::OpenerExt;

pub fn init<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("instance")
        .invoke_handler(tauri::generate_handler![
            instance_remove,
            instance_get_backup_repository_status,
            instance_move_backup_repository,
            instance_get_backup_config,
            instance_normalize_backup_exclusion,
            instance_enable_backups,
            instance_update_backup_exclusions,
            instance_disable_backups,
            instance_start_backup,
            instance_cancel_backup,
            instance_list_backup_operations,
            instance_list_backups,
            instance_delete_backup,
            instance_get_backup_restore_preview,
            instance_restore_backup,
            instance_get_backup_delete_summary,
            instance_create_direct_link,
            instance_sync_direct_links,
            instance_get,
            instance_get_many,
            instance_list,
            instance_set_pinned,
            instance_get_daily_playtime,
            instance_get_daily_playtime_details,
            instance_get_projects,
            instance_get_installed_project_ids,
            instance_get_install_candidates,
            instance_content,
            instance_get_content_items,
            instance_get_content_items_by_paths,
            instance_get_content_snapshot,
            instance_refresh_content,
            instance_plan_content_updates,
            instance_apply_content_update_plan,
            instance_plan_upgrade,
            instance_get_upgrade_plan,
            instance_discard_upgrade_plan,
            instance_update_upgrade_resolution,
            instance_update_upgrade_resolutions,
            instance_reset_upgrade_resolution,
            instance_select_upgrade_solution,
            instance_resolve_custom_upgrade_solution,
            instance_execute_upgrade,
            instance_get_post_upgrade_notice,
            instance_dismiss_post_upgrade_notice,
            instance_get_dependencies_as_content_items,
            instance_get_linked_modpack_info,
            instance_get_linked_modpack_content,
            instance_get_optimal_jre_key,
            instance_list_core_components,
            instance_add_core_jar_mod,
            instance_replace_core_jar,
            instance_move_core_component,
            instance_set_core_component_enabled,
            instance_remove_core_component,
            instance_restore_core_component,
            instance_preview_core_jar,
            instance_install_mcarchive_modloader,
            instance_import_mcarchive_modloader,
            instance_get_full_path,
            instance_get_mod_full_path,
            instance_check_installed,
            instance_queue_content_batch,
            instance_queue_curseforge_world,
            instance_add_project_from_path,
            instance_import_world_save,
            instance_install_datapack_to_world,
            instance_install_datapack_to_world_bytes,
            instance_toggle_disable_project,
            instance_toggle_content_entry,
            instance_toggle_content_entries,
            instance_rollback_project,
            instance_remove_project,
            instance_remove_content_entry,
            instance_queue_content_change,
            instance_restore_pack_member_default,
            instance_update_managed_modrinth_version,
            instance_repair_managed_modrinth,
            instance_run,
            instance_kill,
            instance_edit,
            instance_cache_icon,
            instance_edit_icon,
            instance_list_screenshots,
            instance_list_all_screenshots,
            instance_list_synced_screenshots,
            instance_save_edited_screenshot,
            instance_list_screenshot_groups,
            instance_create_screenshot_group,
            instance_rename_screenshot_group,
            instance_delete_screenshot_group,
            instance_set_screenshot_group_memberships,
            instance_import_screenshot_groups,
            instance_delete_screenshots,
            instance_export_screenshots,
            instance_move_screenshots,
            instance_open_screenshot,
            instance_get_synced_options,
            instance_get_initialized_synced_options,
            instance_get_synced_options_overview,
            instance_get_synced_option_capabilities,
            instance_get_synced_option_join_preview,
            instance_set_synced_option,
            instance_set_instance_synced_option,
            instance_get_synced_command_history,
            instance_set_synced_command_history,
            instance_list_synced_servers,
            instance_update_synced_server,
            instance_remove_synced_server,
            instance_desync_server,
            instance_get_synced_game_options_config,
            instance_preview_synced_game_option_changes,
            instance_save_synced_game_option_changes,
            instance_list_game_options_sync_sources,
            instance_get_game_setting_locale_labels,
            instance_get_local_game_options_config,
            instance_preview_local_game_option_changes,
            instance_save_local_game_option_changes,
            instance_get_pack_sync_preview,
            instance_sync_pack,
            instance_desync_pack,
            instance_list_synced_packs,
            instance_upload_synced_pack,
            instance_set_synced_pack_enabled,
            instance_remove_synced_pack,
            instance_list_groups,
            instance_create_group,
            instance_rename_group,
            instance_delete_group,
            instance_set_group_order,
            instance_set_group_memberships,
            instance_update_group_memberships,
        ])
        .build()
}

#[tauri::command]
pub async fn instance_get_synced_options()
-> Result<mctrin::instance::GlobalSyncedOptions> {
    Ok(mctrin::instance::get_global_synced_options().await?)
}

#[tauri::command]
pub async fn instance_get_initialized_synced_options()
-> Result<mctrin::instance::GlobalSyncedOptions> {
    Ok(mctrin::instance::get_initialized_synced_options().await?)
}

#[tauri::command]
pub async fn instance_get_synced_game_options_config()
-> Result<mctrin::instance::GameSettingsEditorState> {
    Ok(mctrin::instance::get_synced_game_options_config().await?)
}

#[tauri::command]
pub async fn instance_preview_synced_game_option_changes(
    request: mctrin::instance::UpdateGameSettingsRequest,
) -> Result<mctrin::instance::GameSettingsEditorState> {
    Ok(mctrin::instance::preview_synced_game_option_changes(request).await?)
}

#[tauri::command]
pub async fn instance_save_synced_game_option_changes(
    request: mctrin::instance::UpdateGameSettingsRequest,
) -> Result<mctrin::instance::SaveGameSettingsResult> {
    Ok(mctrin::instance::save_synced_game_option_changes(request).await?)
}

#[tauri::command]
pub async fn instance_list_game_options_sync_sources()
-> Result<Vec<mctrin::instance::GameOptionsSourceCandidate>> {
    Ok(mctrin::instance::list_game_options_sync_sources().await?)
}

#[tauri::command]
pub async fn instance_get_game_setting_locale_labels(
    instance_id: Option<String>,
    locale: String,
    option_ids: Vec<String>,
    refresh_sources: bool,
) -> Result<mctrin::instance::GameSettingLocaleLabels> {
    Ok(mctrin::instance::get_game_setting_locale_labels(
        instance_id.as_deref(),
        &locale,
        option_ids,
        refresh_sources,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_get_local_game_options_config(
    instance_id: String,
) -> Result<mctrin::instance::GameSettingsEditorState> {
    Ok(mctrin::instance::get_local_game_options_config(&instance_id).await?)
}

#[tauri::command]
pub async fn instance_preview_local_game_option_changes(
    instance_id: String,
    request: mctrin::instance::UpdateGameSettingsRequest,
) -> Result<mctrin::instance::GameSettingsEditorState> {
    Ok(mctrin::instance::preview_local_game_option_changes(
        &instance_id,
        request,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_save_local_game_option_changes(
    instance_id: String,
    request: mctrin::instance::UpdateGameSettingsRequest,
) -> Result<mctrin::instance::SaveGameSettingsResult> {
    Ok(
        mctrin::instance::save_local_game_option_changes(&instance_id, request)
            .await?,
    )
}

#[tauri::command]
pub async fn instance_get_pack_sync_preview(
    instance_id: String,
    project_path: String,
) -> Result<mctrin::instance::PackSyncPreview> {
    Ok(
        mctrin::instance::get_pack_sync_preview(&instance_id, &project_path)
            .await?,
    )
}

#[tauri::command]
pub async fn instance_sync_pack(
    instance_id: String,
    project_path: String,
) -> Result<()> {
    Ok(mctrin::instance::sync_pack(&instance_id, &project_path).await?)
}

#[tauri::command]
pub async fn instance_desync_pack(
    instance_id: String,
    pack_id: String,
    mode: mctrin::instance::DesyncServerMode,
) -> Result<()> {
    Ok(mctrin::instance::desync_pack(&instance_id, &pack_id, mode).await?)
}

#[tauri::command]
pub async fn instance_list_synced_packs(
    project_type: mctrin::data::ProjectType,
) -> Result<Vec<mctrin::data::ContentItem>> {
    Ok(mctrin::instance::list_synced_packs(project_type).await?)
}

#[tauri::command]
pub async fn instance_upload_synced_pack(
    path: PathBuf,
    project_type: mctrin::data::ProjectType,
    game_versions: Vec<String>,
) -> Result<()> {
    Ok(
        mctrin::instance::upload_synced_pack(path, project_type, game_versions)
            .await?,
    )
}

#[tauri::command]
pub async fn instance_set_synced_pack_enabled(
    pack_id: String,
    enabled: bool,
) -> Result<()> {
    Ok(mctrin::instance::set_synced_pack_enabled(&pack_id, enabled).await?)
}

#[tauri::command]
pub async fn instance_remove_synced_pack(pack_id: String) -> Result<()> {
    Ok(mctrin::instance::remove_synced_pack(&pack_id).await?)
}

#[tauri::command]
pub async fn instance_get_synced_options_overview(
    instance_id: String,
) -> Result<mctrin::instance::SyncedOptionsOverview> {
    Ok(mctrin::instance::get_synced_options_overview(&instance_id).await?)
}

#[tauri::command]
pub async fn instance_get_synced_option_capabilities(
    instance_id: String,
) -> Result<Vec<mctrin::instance::SyncedOptionCapability>> {
    Ok(mctrin::instance::get_synced_option_capabilities(&instance_id).await?)
}

#[tauri::command]
pub async fn instance_get_synced_option_join_preview(
    instance_id: String,
    option: mctrin::instance::SyncedOption,
) -> Result<mctrin::instance::SyncedOptionJoinPreview> {
    Ok(
        mctrin::instance::get_synced_option_join_preview(&instance_id, option)
            .await?,
    )
}

#[tauri::command]
pub async fn instance_set_synced_option(
    option: mctrin::instance::SyncedOption,
    enabled: bool,
    base_instance_id: Option<String>,
) -> Result<mctrin::instance::GlobalSyncedOptions> {
    Ok(mctrin::instance::set_global_synced_option(
        option,
        enabled,
        base_instance_id.as_deref(),
    )
    .await?)
}

#[tauri::command]
pub async fn instance_set_instance_synced_option(
    instance_id: String,
    option: mctrin::instance::SyncedOption,
    enabled: bool,
    resolution: Option<mctrin::instance::SyncedOptionJoinResolution>,
) -> Result<Instance> {
    let metadata = mctrin::instance::set_instance_synced_option(
        &instance_id,
        option,
        enabled,
        resolution,
    )
    .await?;
    instance_from_metadata(metadata).await
}

#[tauri::command]
pub async fn instance_get_synced_command_history() -> Result<String> {
    Ok(mctrin::instance::get_synced_command_history().await?)
}

#[tauri::command]
pub async fn instance_set_synced_command_history(
    contents: String,
) -> Result<String> {
    Ok(mctrin::instance::set_synced_command_history(&contents).await?)
}

#[tauri::command]
pub async fn instance_list_synced_servers()
-> Result<Vec<mctrin::instance::SyncedServer>> {
    Ok(mctrin::instance::list_synced_servers().await?)
}
#[tauri::command]
pub async fn instance_update_synced_server(
    server: mctrin::instance::SyncedServer,
) -> Result<()> {
    mctrin::instance::update_synced_server(server).await?;
    Ok(())
}
#[tauri::command]
pub async fn instance_remove_synced_server(id: String) -> Result<()> {
    mctrin::instance::remove_synced_server(&id).await?;
    Ok(())
}

#[tauri::command]
pub async fn instance_desync_server(
    instance_id: String,
    server_id: String,
    mode: mctrin::instance::DesyncServerMode,
) -> Result<()> {
    mctrin::instance::desync_server(&instance_id, &server_id, mode).await?;
    Ok(())
}

#[derive(Serialize, Debug, Clone)]
pub struct Instance {
    pub id: String,
    pub path: String,
    pub install_stage: String,
    pub launcher_feature_version: String,
    pub name: String,
    pub icon_path: Option<String>,
    pub game_version: String,
    pub protocol_version: Option<u32>,
    pub loader: ModLoader,
    pub loader_version: Option<String>,
    pub loader_components: Vec<mctrin::data::LoaderComponent>,
    pub groups: Vec<String>,
    pub link: Option<InstanceLink>,
    pub update_channel: ReleaseChannel,
    pub created: chrono::DateTime<chrono::Utc>,
    pub modified: chrono::DateTime<chrono::Utc>,
    pub last_played: Option<chrono::DateTime<chrono::Utc>>,
    pub pinned_at: Option<chrono::DateTime<chrono::Utc>>,
    pub submitted_time_played: u64,
    pub recent_time_played: u64,
    pub java_path: Option<String>,
    pub extra_launch_args: Option<Vec<String>>,
    pub custom_env_vars: Option<Vec<(String, String)>>,
    pub memory: Option<MemorySettings>,
    pub force_fullscreen: Option<bool>,
    pub maximize_window: Option<bool>,
    pub game_resolution: Option<WindowSize>,
    pub window_title: Option<String>,
    pub launch_preparation_timeout: Option<u64>,
    pub hooks: Hooks,
    pub symlink_target: Option<String>,
    pub game_dir_override: Option<String>,
    pub linked_launcher: Option<String>,
    pub linked_launcher_root: Option<String>,
    pub linked_dot_minecraft: Option<String>,
    pub linked_version_id: Option<String>,
    pub linked_version_json_path: Option<String>,
    pub linked_game_dir_mode: Option<String>,
    pub synced_options: mctrin::instance::InstanceSyncedOptions,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateDirectLinkInstanceRequest {
    pub name: Option<String>,
    pub launcher_type: ImportLauncherType,
    pub base_path: PathBuf,
    pub instance_folder: String,
    pub instance_path: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct InstanceRunResult {
    pub process: ProcessMetadata,
    pub gc_notice: Option<mctrin::instance::GcLaunchReport>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InstanceLink {
    ModrinthModpack {
        project_id: String,
        version_id: String,
    },
    // Keep this as "curseforge_modpack" (not "curse_forge_modpack") so it matches
    // the frontend InstanceLink type and the stored link_kind string.
    #[serde(rename = "curseforge_modpack")]
    CurseForgeModpack {
        project_id: String,
        version_id: String,
    },
    ServerProject {
        project_id: String,
    },
    ServerProjectModpack {
        server_project_id: String,
        content_project_id: Option<String>,
        content_version_id: String,
        project_id: Option<String>,
        version_id: Option<String>,
    },
    ImportedModpack {
        project_id: Option<String>,
        version_id: Option<String>,
        name: Option<String>,
        version_number: Option<String>,
        filename: Option<String>,
    },
    SharedInstance {
        shared_instance_id: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EditInstance {
    pub name: Option<String>,

    pub game_version: Option<String>,
    pub loader: Option<ModLoader>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub loader_version: Option<Option<String>>,

    pub groups: Option<Vec<String>>,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub link: Option<Option<InstanceLink>>,
    pub update_channel: Option<ReleaseChannel>,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub java_path: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub extra_launch_args: Option<Option<Vec<String>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub custom_env_vars: Option<Option<Vec<(String, String)>>>,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub memory: Option<Option<MemorySettings>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub force_fullscreen: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub maximize_window: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub game_resolution: Option<Option<WindowSize>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub window_title: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub launch_preparation_timeout: Option<Option<u64>>,
    pub hooks: Option<Hooks>,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub game_dir_override: Option<Option<String>>,
}

impl From<InstanceMetadata> for Instance {
    fn from(metadata: InstanceMetadata) -> Self {
        Self {
            id: metadata.instance.id,
            path: metadata.instance.path,
            install_stage: metadata.instance.install_stage.as_str().to_string(),
            launcher_feature_version: metadata
                .instance
                .launcher_feature_version
                .as_str()
                .to_string(),
            name: metadata.instance.name,
            icon_path: metadata.instance.icon_path,
            game_version: metadata.applied_content_set.game_version,
            protocol_version: metadata.applied_content_set.protocol_version,
            loader: metadata.applied_content_set.loader,
            loader_version: metadata.applied_content_set.loader_version,
            loader_components: metadata.loader_components,
            groups: metadata.groups,
            link: InstanceLink::from_core(metadata.link),
            update_channel: metadata.instance.update_channel,
            created: metadata.instance.created,
            modified: metadata.instance.modified,
            last_played: metadata.instance.last_played,
            pinned_at: metadata.instance.pinned_at,
            submitted_time_played: metadata.instance.submitted_time_played,
            recent_time_played: metadata.instance.recent_time_played,
            java_path: metadata.launch_overrides.java_path,
            extra_launch_args: metadata.launch_overrides.extra_launch_args,
            custom_env_vars: metadata.launch_overrides.custom_env_vars,
            memory: metadata.launch_overrides.memory,
            force_fullscreen: metadata.launch_overrides.force_fullscreen,
            maximize_window: metadata.launch_overrides.maximize_window,
            game_resolution: metadata.launch_overrides.game_resolution,
            window_title: metadata.launch_overrides.window_title,
            launch_preparation_timeout: metadata
                .launch_overrides
                .launch_preparation_timeout,
            hooks: metadata.launch_overrides.hooks,
            symlink_target: metadata.instance.symlink_target,
            game_dir_override: metadata.instance.game_dir_override,
            linked_launcher: metadata.instance.linked_launcher,
            linked_launcher_root: metadata.instance.linked_launcher_root,
            linked_dot_minecraft: metadata.instance.linked_dot_minecraft,
            linked_version_id: metadata.instance.linked_version_id,
            linked_version_json_path: metadata
                .instance
                .linked_version_json_path,
            linked_game_dir_mode: metadata.instance.linked_game_dir_mode,
            synced_options: metadata.synced_options,
        }
    }
}

impl InstanceLink {
    fn from_core(link: CoreInstanceLink) -> Option<Self> {
        match link {
            CoreInstanceLink::Unmanaged => None,
            CoreInstanceLink::ModrinthModpack {
                project_id,
                version_id,
            } => Some(Self::ModrinthModpack {
                project_id,
                version_id,
            }),
            CoreInstanceLink::CurseForgeModpack {
                project_id,
                version_id,
            } => Some(Self::CurseForgeModpack {
                project_id,
                version_id,
            }),
            CoreInstanceLink::ServerProject { project_id } => {
                Some(Self::ServerProject { project_id })
            }
            CoreInstanceLink::ServerProjectModpack {
                server_project_id,
                content_project_id,
                content_version_id,
            } => Some(Self::ServerProjectModpack {
                project_id: Some(server_project_id.clone()),
                version_id: Some(content_version_id.clone()),
                server_project_id,
                content_project_id: Some(content_project_id),
                content_version_id,
            }),
            CoreInstanceLink::ImportedModpack {
                project_id,
                version_id,
                name,
                version_number,
                filename,
            } => Some(Self::ImportedModpack {
                project_id,
                version_id,
                name,
                version_number,
                filename,
            }),
            CoreInstanceLink::SharedInstance { shared_instance_id } => {
                Some(Self::SharedInstance {
                    shared_instance_id: shared_instance_id.to_string(),
                })
            }
        }
    }

    pub(crate) fn into_core(self) -> Result<CoreInstanceLink> {
        match self {
            Self::ModrinthModpack {
                project_id,
                version_id,
            } => Ok(CoreInstanceLink::ModrinthModpack {
                project_id,
                version_id,
            }),
            Self::CurseForgeModpack {
                project_id,
                version_id,
            } => Ok(CoreInstanceLink::CurseForgeModpack {
                project_id,
                version_id,
            }),
            Self::ServerProject { project_id } => {
                Ok(CoreInstanceLink::ServerProject { project_id })
            }
            Self::ServerProjectModpack {
                server_project_id,
                content_project_id,
                content_version_id,
                ..
            } => Ok(CoreInstanceLink::ServerProjectModpack {
                server_project_id,
                content_project_id: content_project_id.unwrap_or_default(),
                content_version_id,
            }),
            Self::ImportedModpack {
                project_id,
                version_id,
                name,
                version_number,
                filename,
            } => Ok(CoreInstanceLink::ImportedModpack {
                project_id,
                version_id,
                name,
                version_number,
                filename,
            }),
            Self::SharedInstance { shared_instance_id } => {
                Ok(CoreInstanceLink::SharedInstance {
                    shared_instance_id: shared_instance_id.parse().map_err(
                        |err| {
                            mctrin::Error::from(mctrin::ErrorKind::InputError(
                                format!("Invalid shared instance id: {err}"),
                            ))
                        },
                    )?,
                })
            }
        }
    }
}

fn edit_to_core(edit_instance: EditInstance) -> Result<CoreEditInstance> {
    Ok(CoreEditInstance {
        install_stage: None,
        launcher_feature_version: None,
        name: edit_instance.name,
        icon_path: None,
        update_channel: edit_instance.update_channel,
        groups: edit_instance.groups,
        link: edit_instance
            .link
            .map(|link| match link {
                Some(link) => link.into_core(),
                None => Ok(CoreInstanceLink::Unmanaged),
            })
            .transpose()?,
        launch_overrides: Some(InstanceLaunchOverridesPatch {
            java_path: edit_instance.java_path,
            extra_launch_args: edit_instance.extra_launch_args,
            custom_env_vars: edit_instance.custom_env_vars,
            memory: edit_instance.memory,
            force_fullscreen: edit_instance.force_fullscreen,
            maximize_window: edit_instance.maximize_window,
            game_resolution: edit_instance.game_resolution,
            window_title: edit_instance.window_title,
            launch_preparation_timeout: edit_instance
                .launch_preparation_timeout,
            hooks: edit_instance.hooks,
        }),
        content_set_patch: Some(AppliedContentSetPatch {
            source_kind: None,
            game_version: edit_instance.game_version,
            protocol_version: Some(None),
            loader: edit_instance.loader,
            loader_version: edit_instance.loader_version,
        }),
        last_played: None,
        submitted_time_played: None,
        recent_time_played: None,
        symlink_target: None,
        game_dir_override: edit_instance.game_dir_override,
    })
}

const LOCAL_INSTANCE_ICON_MAX_DIMENSION: u32 = 256;

async fn instance_from_metadata(
    metadata: InstanceMetadata,
) -> Result<Instance> {
    let mut instance = Instance::from(metadata);
    if instance.icon_path.is_none()
        && let Ok(Some(local_icon)) = local_instance_icon_path(
            &instance.id,
            LOCAL_INSTANCE_ICON_MAX_DIMENSION,
        )
        .await
    {
        instance.icon_path = Some(local_icon);
    }
    Ok(instance)
}

#[tauri::command]
pub async fn instance_remove(instance_id: &str) -> Result<()> {
    mctrin::instance::remove(instance_id).await?;
    Ok(())
}

#[tauri::command]
pub async fn instance_get_backup_repository_status()
-> Result<mctrin::instance::BackupRepositoryStatus> {
    Ok(mctrin::instance::get_backup_repository_status().await?)
}

#[tauri::command]
pub async fn instance_move_backup_repository(
    destination: PathBuf,
) -> Result<uuid::Uuid> {
    Ok(mctrin::instance::move_backup_repository(destination).await?)
}

#[tauri::command]
pub async fn instance_get_backup_config(
    instance_id: &str,
) -> Result<mctrin::instance::InstanceBackupConfig> {
    Ok(mctrin::instance::get_backup_config(instance_id).await?)
}

#[tauri::command]
pub async fn instance_enable_backups(
    instance_id: &str,
    excluded_paths: Vec<mctrin::instance::BackupExclusion>,
) -> Result<mctrin::instance::InstanceBackupConfig> {
    Ok(mctrin::instance::enable_backups(instance_id, excluded_paths).await?)
}

#[tauri::command]
pub async fn instance_normalize_backup_exclusion(
    instance_id: &str,
    selected_path: PathBuf,
    kind: mctrin::instance::BackupExclusionKind,
) -> Result<mctrin::instance::BackupExclusion> {
    Ok(mctrin::instance::get_backup_exclusion_from_path(
        instance_id,
        selected_path,
        kind,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_update_backup_exclusions(
    instance_id: &str,
    excluded_paths: Vec<mctrin::instance::BackupExclusion>,
) -> Result<mctrin::instance::InstanceBackupConfig> {
    Ok(
        mctrin::instance::update_backup_exclusions(instance_id, excluded_paths)
            .await?,
    )
}

#[tauri::command]
pub async fn instance_disable_backups(instance_id: &str) -> Result<()> {
    Ok(mctrin::instance::disable_backups(instance_id).await?)
}

#[tauri::command]
pub async fn instance_start_backup(instance_id: String) -> Result<uuid::Uuid> {
    Ok(mctrin::instance::start_backup_snapshot(instance_id).await?)
}

#[tauri::command]
pub async fn instance_cancel_backup(operation_id: uuid::Uuid) -> Result<bool> {
    Ok(mctrin::instance::cancel_backup(operation_id).await?)
}

#[tauri::command]
pub async fn instance_list_backup_operations(
    instance_id: Option<String>,
    active_only: bool,
) -> Result<Vec<mctrin::instance::BackupOperation>> {
    Ok(mctrin::instance::list_backup_operations(
        instance_id.as_deref(),
        active_only,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_list_backups(
    instance_id: &str,
) -> Result<Vec<mctrin::instance::BackupSnapshot>> {
    Ok(mctrin::instance::list_backup_snapshots(instance_id).await?)
}

#[tauri::command]
pub async fn instance_delete_backup(snapshot_id: &str) -> Result<()> {
    Ok(mctrin::instance::delete_backup_snapshot(snapshot_id).await?)
}

#[tauri::command]
pub async fn instance_get_backup_restore_preview(
    snapshot_id: &str,
) -> Result<uuid::Uuid> {
    Ok(mctrin::instance::get_backup_restore_preview(snapshot_id).await?)
}

#[tauri::command]
pub async fn instance_restore_backup(
    snapshot_id: &str,
    plan_token: &str,
) -> Result<uuid::Uuid> {
    Ok(mctrin::instance::restore_snapshot(snapshot_id, plan_token).await?)
}

#[tauri::command]
pub async fn instance_get_backup_delete_summary(
    instance_id: &str,
) -> Result<mctrin::instance::BackupDeleteSummary> {
    Ok(mctrin::instance::get_backup_delete_summary(instance_id).await?)
}

#[tauri::command]
pub async fn instance_create_direct_link(
    request: CreateDirectLinkInstanceRequest,
) -> Result<Instance> {
    let metadata = mctrin::instance::create_with_direct_link(
        mctrin::data::CreateDirectLinkInstance {
            name: request.name,
            launcher_type: request.launcher_type,
            base_path: request.base_path,
            instance_folder: request.instance_folder,
            instance_path: request.instance_path,
            game_dir_mode: None,
        },
    )
    .await?;
    instance_from_metadata(metadata).await
}

#[tauri::command]
pub async fn instance_sync_direct_links(
    roots: Vec<mctrin::data::ExternalMinecraftRoot>,
) -> Result<mctrin::data::DirectLinkSyncReport> {
    Ok(mctrin::instance::sync_direct_links(roots).await?)
}

#[tauri::command]
pub async fn instance_get(instance_id: &str) -> Result<Option<Instance>> {
    let Some(metadata) = mctrin::instance::get(instance_id).await? else {
        return Ok(None);
    };
    Ok(Some(instance_from_metadata(metadata).await?))
}

#[tauri::command]
pub async fn instance_get_many(
    instance_ids: Vec<String>,
) -> Result<Vec<Instance>> {
    let ids = instance_ids.iter().map(|x| &**x).collect::<Vec<&str>>();
    instances_from_metadata(mctrin::instance::get_many(&ids).await?).await
}

#[tauri::command]
pub async fn instance_list() -> Result<Vec<Instance>> {
    instances_from_metadata(mctrin::instance::list().await?).await
}

async fn instances_from_metadata(
    metadata: Vec<InstanceMetadata>,
) -> Result<Vec<Instance>> {
    futures::stream::iter(metadata.into_iter().map(instance_from_metadata))
        .buffered(4)
        .try_collect()
        .await
}

#[tauri::command]
pub async fn instance_set_pinned(
    instance_id: String,
    pinned: bool,
) -> Result<Instance> {
    let metadata = mctrin::instance::set_pinned(&instance_id, pinned).await?;
    instance_from_metadata(metadata).await
}

#[tauri::command]
pub async fn instance_get_daily_playtime(
    start_date: String,
    end_date: String,
) -> Result<Vec<mctrin::instance::DailyPlaytime>> {
    let start_date = NaiveDate::parse_from_str(&start_date, "%Y-%m-%d")
        .map_err(|error| {
            mctrin::ErrorKind::InputError(format!(
                "Invalid start date: {error}"
            ))
            .as_error()
        })?;
    let end_date =
        NaiveDate::parse_from_str(&end_date, "%Y-%m-%d").map_err(|error| {
            mctrin::ErrorKind::InputError(format!("Invalid end date: {error}"))
                .as_error()
        })?;

    Ok(mctrin::instance::get_daily_playtime(start_date, end_date).await?)
}

#[tauri::command]
pub async fn instance_get_daily_playtime_details(
    date: String,
) -> Result<Vec<mctrin::instance::DailyPlaytimeEntry>> {
    let date =
        NaiveDate::parse_from_str(&date, "%Y-%m-%d").map_err(|error| {
            mctrin::ErrorKind::InputError(format!("Invalid date: {error}"))
                .as_error()
        })?;

    Ok(mctrin::instance::get_daily_playtime_details(date).await?)
}

#[tauri::command]
pub async fn instance_get_projects(
    instance_id: &str,
    cache_behaviour: Option<CacheBehaviour>,
) -> Result<DashMap<String, ContentFile>> {
    Ok(mctrin::instance::get_projects(instance_id, cache_behaviour).await?)
}

#[tauri::command]
pub async fn instance_get_installed_project_ids(
    instance_id: &str,
) -> Result<Vec<String>> {
    Ok(mctrin::instance::get_installed_project_ids(instance_id).await?)
}

#[tauri::command]
pub async fn instance_get_install_candidates(
    project_id: &str,
    project_type: ProjectType,
    targets: Vec<InstanceInstallTarget>,
) -> Result<Vec<InstanceInstallCandidate>> {
    Ok(mctrin::instance::get_install_candidates(
        project_id,
        project_type,
        targets,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_content(
    instance_id: &str,
    cache_behaviour: Option<CacheBehaviour>,
) -> Result<Vec<ContentItem>> {
    instance_get_content_items(instance_id, cache_behaviour).await
}

#[tauri::command]
pub async fn instance_get_content_items(
    instance_id: &str,
    cache_behaviour: Option<CacheBehaviour>,
) -> Result<Vec<ContentItem>> {
    Ok(
        mctrin::instance::get_content_items(instance_id, cache_behaviour)
            .await?,
    )
}

#[tauri::command]
pub async fn instance_get_content_items_by_paths(
    instance_id: &str,
    paths: Vec<String>,
    cache_behaviour: Option<CacheBehaviour>,
) -> Result<Vec<ContentItem>> {
    Ok(mctrin::instance::get_content_items_by_paths(
        instance_id,
        paths,
        cache_behaviour,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_get_content_snapshot(
    instance_id: &str,
    defer_enrichment: Option<bool>,
) -> Result<mctrin::data::InstanceContentSnapshot> {
    if defer_enrichment.unwrap_or(false) {
        return Ok(mctrin::instance::get_content_snapshot_for_display(
            instance_id,
        )
        .await?);
    }
    Ok(mctrin::instance::get_content_snapshot(instance_id).await?)
}

#[tauri::command]
pub async fn instance_refresh_content(
    instance_id: &str,
) -> Result<mctrin::data::InstanceContentSnapshot> {
    Ok(mctrin::instance::refresh_content(instance_id).await?)
}

#[tauri::command]
pub async fn instance_plan_content_updates(
    instance_id: &str,
    scope: mctrin::data::ContentUpdateScope,
    target: Option<&str>,
) -> Result<mctrin::data::ContentUpdatePlan> {
    Ok(
        mctrin::instance::plan_content_updates(instance_id, scope, target)
            .await?,
    )
}

#[tauri::command]
pub async fn instance_apply_content_update_plan(
    plan_id: &str,
    resolutions: Vec<mctrin::data::ContentUpdateResolution>,
) -> Result<mctrin::data::InstanceContentSnapshot> {
    Ok(
        mctrin::instance::apply_content_update_plan(plan_id, resolutions)
            .await?,
    )
}

#[tauri::command]
pub async fn instance_plan_upgrade(
    instance_id: &str,
    target_environment: mctrin::data::InstanceUpgradeEnvironment,
) -> Result<mctrin::data::InstanceUpgradePlan> {
    Ok(
        mctrin::instance::plan_instance_upgrade(
            instance_id,
            target_environment,
        )
        .await?,
    )
}

#[tauri::command]
pub async fn instance_discard_upgrade_plan(plan_id: &str) -> Result<()> {
    mctrin::instance::discard_instance_upgrade_plan(plan_id);
    Ok(())
}

#[tauri::command]
pub async fn instance_get_upgrade_plan(
    plan_id: &str,
) -> Result<mctrin::data::InstanceUpgradePlan> {
    Ok(mctrin::instance::get_instance_upgrade_plan(plan_id).await?)
}

#[tauri::command]
pub async fn instance_update_upgrade_resolution(
    plan_id: &str,
    resolution: mctrin::data::InstanceUpgradeResolution,
) -> Result<mctrin::data::InstanceUpgradePlan> {
    Ok(mctrin::instance::update_instance_upgrade_resolution(
        plan_id, resolution,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_update_upgrade_resolutions(
    plan_id: &str,
    resolutions: Vec<mctrin::data::InstanceUpgradeResolution>,
) -> Result<mctrin::data::InstanceUpgradeResolutionBatchResult> {
    Ok(mctrin::instance::update_instance_upgrade_resolutions(
        plan_id,
        resolutions,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_reset_upgrade_resolution(
    plan_id: &str,
    content_id: &str,
) -> Result<mctrin::data::InstanceUpgradePlan> {
    Ok(
        mctrin::instance::reset_instance_upgrade_resolution(
            plan_id, content_id,
        )
        .await?,
    )
}

#[tauri::command]
pub async fn instance_select_upgrade_solution(
    plan_id: &str,
    choice: mctrin::data::InstanceUpgradeSolutionChoice,
) -> Result<mctrin::data::InstanceUpgradePlan> {
    Ok(
        mctrin::instance::select_instance_upgrade_solution(plan_id, choice)
            .await?,
    )
}

#[tauri::command]
pub async fn instance_resolve_custom_upgrade_solution(
    plan_id: &str,
    fixed_constraints: Vec<mctrin::data::InstanceUpgradeFixedConstraint>,
) -> Result<mctrin::data::InstanceUpgradePlan> {
    Ok(mctrin::instance::resolve_custom_instance_upgrade_solution(
        plan_id,
        fixed_constraints,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_execute_upgrade(
    plan_id: &str,
    create_full_backup: bool,
    shared_upgrade_mode: mctrin::install::SharedUpgradeMode,
    display_names: Option<mctrin::install::InstanceUpgradeDisplayNames>,
) -> Result<mctrin::install::InstallJobSnapshot> {
    Ok(mctrin::instance::execute_instance_upgrade(
        plan_id,
        create_full_backup,
        shared_upgrade_mode,
        display_names.unwrap_or_default(),
    )
    .await?)
}

#[tauri::command]
pub async fn instance_get_post_upgrade_notice(
    instance_id: &str,
) -> Result<Option<mctrin::data::InstancePostUpgradeNotice>> {
    Ok(mctrin::instance::get_instance_post_upgrade_notice(instance_id).await?)
}

#[tauri::command]
pub async fn instance_dismiss_post_upgrade_notice(
    instance_id: &str,
) -> Result<()> {
    Ok(
        mctrin::instance::dismiss_instance_post_upgrade_notice(instance_id)
            .await?,
    )
}

#[tauri::command]
pub async fn instance_get_dependencies_as_content_items(
    dependencies: Vec<Dependency>,
    cache_behaviour: Option<CacheBehaviour>,
) -> Result<Vec<ContentItem>> {
    Ok(mctrin::instance::get_dependencies_as_content_items(
        dependencies,
        cache_behaviour,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_get_linked_modpack_info(
    instance_id: &str,
    cache_behaviour: Option<CacheBehaviour>,
) -> Result<Option<LinkedModpackInfo>> {
    Ok(
        mctrin::instance::get_linked_modpack_info(instance_id, cache_behaviour)
            .await?,
    )
}

#[tauri::command]
pub async fn instance_get_linked_modpack_content(
    instance_id: &str,
    cache_behaviour: Option<CacheBehaviour>,
) -> Result<Vec<ContentItem>> {
    Ok(mctrin::instance::get_linked_modpack_content(
        instance_id,
        cache_behaviour,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_get_full_path<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    instance_id: &str,
) -> Result<PathBuf> {
    let path = mctrin::instance::get_full_path(instance_id).await?;
    crate::api::files::ensure_browsable(&app, &path);
    Ok(path)
}

#[tauri::command]
pub async fn instance_get_mod_full_path<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    instance_id: &str,
    project_path: &str,
) -> Result<PathBuf> {
    let path =
        mctrin::instance::get_mod_full_path(instance_id, project_path).await?;
    if let Some(parent) = path.parent() {
        crate::api::files::ensure_browsable(&app, parent);
    }
    Ok(path)
}

#[derive(Serialize, Debug, Clone)]
pub struct InstanceScreenshot {
    pub id: String,
    pub instance_id: String,
    pub instance_name: String,
    pub file_name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub modified_at: i64,
    pub group_id: Option<String>,
    pub path: PathBuf,
    pub url: String,
}

fn serialize_screenshot<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    s: mctrin::instance::InstanceScreenshot,
) -> Result<InstanceScreenshot> {
    if let Some(parent) = s.path.parent() {
        crate::api::files::ensure_browsable(app, parent);
    }
    let mut url = super::utils::tauri_convert_file_src(&s.path)?;
    app.asset_protocol_scope().allow_file(&s.path)?;
    app.fs_scope().allow_file(&s.path)?;
    url.query_pairs_mut()
        .append_pair("revision", &s.modified_at.to_string());
    Ok(InstanceScreenshot {
        id: s.id,
        instance_id: s.instance_id,
        instance_name: s.instance_name,
        file_name: s.file_name,
        created_at: s.created_at,
        modified_at: s.modified_at,
        group_id: s.group_id,
        path: s.path,
        url: url.to_string(),
    })
}
fn serialize_screenshots<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    items: Vec<mctrin::instance::InstanceScreenshot>,
) -> Result<Vec<InstanceScreenshot>> {
    items
        .into_iter()
        .map(|s| serialize_screenshot(app, s))
        .collect()
}

#[tauri::command]
pub async fn instance_list_screenshots<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    instance_id: &str,
) -> Result<Vec<InstanceScreenshot>> {
    serialize_screenshots(
        &app,
        mctrin::instance::list_screenshots(instance_id).await?,
    )
}
#[tauri::command]
pub async fn instance_list_all_screenshots<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<ScreenshotScanResult> {
    let result = mctrin::instance::list_all_screenshots_with_warnings().await?;
    Ok(ScreenshotScanResult {
        screenshots: serialize_screenshots(&app, result.screenshots)?,
        skipped_instances: result.skipped_instances,
    })
}
#[derive(serde::Serialize)]
pub struct ScreenshotScanResult {
    screenshots: Vec<InstanceScreenshot>,
    skipped_instances: Vec<String>,
}
#[tauri::command]
pub async fn instance_list_synced_screenshots<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<ScreenshotScanResult> {
    instance_list_all_screenshots(app).await
}
#[tauri::command]
pub async fn instance_save_edited_screenshot<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    key: mctrin::instance::ScreenshotKey,
    bytes: Vec<u8>,
    mode: mctrin::instance::ScreenshotEditSaveMode,
) -> Result<InstanceScreenshot> {
    serialize_screenshot(
        &app,
        mctrin::instance::save_edited_screenshot(key, bytes, mode).await?,
    )
}
#[tauri::command]
pub async fn instance_list_screenshot_groups()
-> Result<Vec<mctrin::instance::ScreenshotGroup>> {
    Ok(mctrin::instance::list_screenshot_groups().await?)
}
#[tauri::command]
pub async fn instance_create_screenshot_group(
    name: String,
    ids: Vec<String>,
) -> Result<mctrin::instance::ScreenshotGroup> {
    Ok(mctrin::instance::create_screenshot_group(name, ids).await?)
}
#[tauri::command]
pub async fn instance_rename_screenshot_group(
    id: String,
    new_name: String,
) -> Result<mctrin::instance::ScreenshotGroup> {
    Ok(mctrin::instance::rename_screenshot_group(id, new_name).await?)
}
#[tauri::command]
pub async fn instance_delete_screenshot_group(id: String) -> Result<()> {
    Ok(mctrin::instance::delete_screenshot_group(id).await?)
}
#[tauri::command]
pub async fn instance_set_screenshot_group_memberships(
    updates: Vec<mctrin::instance::ScreenshotGroupMembershipUpdate>,
) -> Result<()> {
    Ok(mctrin::instance::set_screenshot_group_memberships(updates).await?)
}
#[tauri::command]
pub async fn instance_import_screenshot_groups(
    groups: Vec<mctrin::instance::ScreenshotGroupImport>,
) -> Result<()> {
    Ok(mctrin::instance::import_screenshot_groups(groups).await?)
}
#[tauri::command]
pub async fn instance_delete_screenshots(
    keys: Vec<mctrin::instance::ScreenshotKey>,
) -> Result<()> {
    Ok(mctrin::instance::delete_screenshots(&keys).await?)
}
#[tauri::command]
pub async fn instance_export_screenshots(
    keys: Vec<mctrin::instance::ScreenshotKey>,
    path: PathBuf,
) -> Result<()> {
    Ok(mctrin::instance::export_screenshots(&keys, path).await?)
}
#[tauri::command]
pub async fn instance_move_screenshots(
    keys: Vec<mctrin::instance::ScreenshotKey>,
    target: &str,
) -> Result<Vec<mctrin::instance::ScreenshotKey>> {
    Ok(mctrin::instance::move_screenshots(&keys, target).await?)
}
#[tauri::command]
pub async fn instance_open_screenshot<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    key: mctrin::instance::ScreenshotKey,
) -> Result<()> {
    let path = mctrin::instance::get_screenshot_path(&key).await?;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(())
}

#[tauri::command]
pub async fn instance_get_optimal_jre_key(
    instance_id: &str,
) -> Result<Option<JavaVersion>> {
    Ok(mctrin::instance::get_optimal_jre_key(instance_id).await?)
}

#[tauri::command]
pub async fn instance_list_core_components(
    instance_id: &str,
) -> Result<Vec<mctrin::data::CoreComponent>> {
    Ok(mctrin::instance::list_core_components(instance_id).await?)
}

#[tauri::command]
pub async fn instance_add_core_jar_mod(
    instance_id: &str,
    source_path: PathBuf,
    target_game_version: String,
    source: Option<mctrin::data::CoreComponentSource>,
) -> Result<mctrin::data::CoreComponent> {
    Ok(mctrin::instance::add_core_jar_mod(
        instance_id,
        source_path,
        target_game_version,
        source,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_replace_core_jar(
    instance_id: &str,
    source_path: PathBuf,
    target_game_version: String,
    source: Option<mctrin::data::CoreComponentSource>,
) -> Result<mctrin::data::CoreComponent> {
    Ok(mctrin::instance::replace_core_jar(
        instance_id,
        source_path,
        target_game_version,
        source,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_move_core_component(
    instance_id: &str,
    component_id: &str,
    direction: i32,
) -> Result<Vec<mctrin::data::CoreComponent>> {
    Ok(mctrin::instance::move_core_component(
        instance_id,
        component_id,
        direction,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_set_core_component_enabled(
    instance_id: &str,
    component_id: &str,
    enabled: bool,
) -> Result<mctrin::data::CoreComponent> {
    Ok(mctrin::instance::set_core_component_enabled(
        instance_id,
        component_id,
        enabled,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_remove_core_component(
    instance_id: &str,
    component_id: &str,
) -> Result<()> {
    mctrin::instance::remove_core_component(instance_id, component_id).await?;
    Ok(())
}

#[tauri::command]
pub async fn instance_restore_core_component(
    instance_id: &str,
    component_id: &str,
) -> Result<mctrin::data::CoreComponent> {
    Ok(
        mctrin::instance::restore_core_component(instance_id, component_id)
            .await?,
    )
}

#[tauri::command]
pub async fn instance_preview_core_jar(
    instance_id: &str,
) -> Result<Option<mctrin::data::CoreJarPreview>> {
    Ok(mctrin::instance::preview_core_jar(instance_id).await?)
}

#[tauri::command]
pub async fn instance_install_mcarchive_modloader(
    instance_id: &str,
    game_version: &str,
) -> Result<mctrin::instance::McArchiveCoreInstallResult> {
    Ok(
        mctrin::instance::install_mcarchive_modloader(
            instance_id,
            game_version,
        )
        .await?,
    )
}

#[tauri::command]
pub async fn instance_import_mcarchive_modloader(
    instance_id: &str,
    game_version: &str,
    source_path: PathBuf,
) -> Result<mctrin::instance::McArchiveCoreInstallResult> {
    Ok(mctrin::instance::import_mcarchive_modloader(
        instance_id,
        game_version,
        source_path,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_check_installed(
    instance_id: &str,
    project_id: &str,
) -> Result<bool> {
    if let Ok(projects) =
        mctrin::instance::get_projects(instance_id, None).await
    {
        Ok(projects.into_iter().any(|(_, project)| {
            project
                .provider_refs
                .iter()
                .any(|reference| match reference {
                    mctrin::data::ContentProviderRef::Modrinth {
                        project_id: id,
                        ..
                    } => project_id == id.as_str(),
                    mctrin::data::ContentProviderRef::CurseForge {
                        project_id: id,
                        ..
                    } => project_id == format!("curseforge:{}", id.get()),
                    mctrin::data::ContentProviderRef::McArchive {
                        project_id: id,
                        ..
                    } => project_id == format!("mcarchive:{id}"),
                })
        }))
    } else {
        Ok(false)
    }
}

#[tauri::command]
pub async fn instance_queue_content_batch(
    request: InstallContentBatchRequest,
) -> Result<mctrin::install::InstallJobSnapshot> {
    Ok(mctrin::instance::queue_content_batch(request).await?)
}

#[tauri::command]
pub async fn instance_queue_curseforge_world(
    request: mctrin::curseforge::CurseForgeWorldInstallRequest,
    display_title: String,
    display_icon: Option<String>,
) -> Result<mctrin::install::InstallJobSnapshot> {
    Ok(mctrin::instance::queue_curseforge_world(
        request,
        display_title,
        display_icon,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_add_project_from_path(
    instance_id: &str,
    project_path: &Path,
    project_type: Option<ProjectType>,
    inner_base: Option<String>,
) -> Result<String> {
    Ok(mctrin::instance::add_project_from_path(
        instance_id,
        project_path,
        project_type,
        inner_base.as_deref(),
    )
    .await?)
}

#[tauri::command]
pub async fn instance_import_world_save(
    instance_id: String,
    source_path: String,
    inner_base: Option<String>,
) -> Result<String> {
    Ok(mctrin::instance::import_world_save(
        &instance_id,
        &std::path::PathBuf::from(&source_path),
        inner_base.as_deref(),
    )
    .await?)
}

#[tauri::command]
pub async fn instance_install_datapack_to_world(
    instance_id: String,
    world_path: String,
    source_path: String,
    inner_base: Option<String>,
) -> Result<String> {
    Ok(mctrin::instance::install_datapack_to_world(
        &instance_id,
        &world_path,
        &std::path::PathBuf::from(&source_path),
        inner_base.as_deref(),
    )
    .await?)
}

#[tauri::command]
pub async fn instance_install_datapack_to_world_bytes(
    instance_id: String,
    world_path: String,
    file_name: String,
    bytes: Vec<u8>,
) -> Result<String> {
    Ok(mctrin::instance::install_datapack_bytes_to_world(
        &instance_id,
        &world_path,
        &file_name,
        bytes,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_toggle_disable_project(
    instance_id: &str,
    project_path: &str,
    desired_enabled: Option<bool>,
) -> Result<String> {
    Ok(mctrin::instance::toggle_disable_project(
        instance_id,
        project_path,
        desired_enabled,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_toggle_content_entry(
    instance_id: &str,
    content_id: &str,
    desired_enabled: Option<bool>,
) -> Result<String> {
    Ok(mctrin::instance::toggle_content_entry(
        instance_id,
        content_id,
        desired_enabled,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_toggle_content_entries(
    instance_id: &str,
    content_ids: Vec<String>,
    desired_enabled: Option<bool>,
) -> Result<Vec<mctrin::instance::ContentToggleResult>> {
    Ok(mctrin::instance::toggle_content_entries(
        instance_id,
        content_ids,
        desired_enabled,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_rollback_project(
    instance_id: &str,
    project_path: &str,
) -> Result<String> {
    Ok(mctrin::instance::rollback_project(instance_id, project_path).await?)
}

#[tauri::command]
pub async fn instance_remove_project(
    instance_id: &str,
    project_path: &str,
) -> Result<()> {
    mctrin::instance::remove_project(instance_id, project_path).await?;
    Ok(())
}

#[tauri::command]
pub async fn instance_remove_content_entry(
    instance_id: &str,
    content_id: &str,
) -> Result<()> {
    mctrin::instance::remove_content_entry(instance_id, content_id).await?;
    Ok(())
}

#[tauri::command]
pub async fn instance_queue_content_change(
    instance_id: &str,
    intent: mctrin::install::ContentChangeIntent,
    display_title: String,
    display_icon: Option<String>,
) -> Result<mctrin::install::InstallJobSnapshot> {
    Ok(mctrin::install::queue_content_change(
        instance_id.to_string(),
        intent,
        display_title,
        display_icon,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_restore_pack_member_default(
    instance_id: &str,
    member_id: &str,
) -> Result<Option<String>> {
    Ok(
        mctrin::instance::restore_pack_member_default(instance_id, member_id)
            .await?,
    )
}

#[tauri::command]
pub async fn instance_update_managed_modrinth_version(
    instance_id: String,
    version_id: String,
) -> Result<mctrin::install::InstallJobSnapshot> {
    Ok(mctrin::instance::update_managed_modrinth_version(
        &instance_id,
        &version_id,
    )
    .await?)
}

#[tauri::command]
pub async fn instance_repair_managed_modrinth(
    instance_id: &str,
) -> Result<mctrin::install::InstallJobSnapshot> {
    Ok(mctrin::instance::repair_managed_modrinth(instance_id).await?)
}

#[tauri::command]
pub async fn instance_run(
    instance_id: &str,
    server_address: Option<String>,
    offline_mode: bool,
    extra_launch_args: Option<Vec<String>>,
    gc_intent: Option<mctrin::instance::GcLaunchIntent>,
) -> Result<InstanceRunResult> {
    let quick_play = match server_address {
        Some(addr) => QuickPlayType::Server(ServerAddress::Unresolved(addr)),
        None => QuickPlayType::None,
    };
    let (process, gc_notice) =
        mctrin::instance::run_with_extra_launch_args_with_gc(
            instance_id,
            quick_play,
            offline_mode,
            extra_launch_args,
            gc_intent,
        )
        .await?;
    Ok(InstanceRunResult { process, gc_notice })
}

#[tauri::command]
pub async fn instance_kill(instance_id: &str) -> Result<()> {
    mctrin::instance::kill(instance_id).await?;
    Ok(())
}

#[tauri::command]
pub async fn instance_edit(
    instance_id: &str,
    edit_instance: EditInstance,
) -> Result<()> {
    mctrin::instance::edit(instance_id, edit_to_core(edit_instance)?).await?;
    Ok(())
}

#[tauri::command]
pub async fn instance_cache_icon(
    icon_name: &str,
    bytes: Vec<u8>,
) -> Result<String> {
    Ok(mctrin::instance::cache_icon(icon_name, bytes).await?)
}

#[tauri::command]
pub async fn instance_edit_icon<R: Runtime>(
    app: AppHandle<R>,
    instance_id: &str,
    icon_path: Option<&Path>,
) -> Result<()> {
    mctrin::instance::edit_icon(instance_id, icon_path).await?;
    let instance =
        mctrin::instance::get(instance_id).await?.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("unknown instance: {instance_id}"),
            )
        })?;
    if let Err(error) = crate::api::shortcuts::sync_instance_shortcut_icon(
        &app,
        instance_id,
        instance.instance.icon_path.as_deref().map(Path::new),
        true,
    )
    .await
    {
        tracing::warn!(
            "failed to update shortcut icon for instance {}: {}",
            instance_id,
            error
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn instance_list_groups() -> Result<Vec<InstanceGroup>> {
    Ok(mctrin::instance::list_groups().await?)
}

#[tauri::command]
pub async fn instance_create_group(name: String) -> Result<InstanceGroup> {
    Ok(mctrin::instance::create_group(name).await?)
}

#[tauri::command]
pub async fn instance_rename_group(
    id: String,
    new_name: String,
) -> Result<InstanceGroup> {
    Ok(mctrin::instance::rename_group(id, new_name).await?)
}

#[tauri::command]
pub async fn instance_delete_group(id: String) -> Result<()> {
    Ok(mctrin::instance::delete_group(id).await?)
}

#[tauri::command]
pub async fn instance_set_group_order(group_ids: Vec<String>) -> Result<()> {
    Ok(mctrin::instance::set_group_order(group_ids).await?)
}

#[tauri::command]
pub async fn instance_set_group_memberships(
    updates: Vec<InstanceGroupMembershipUpdate>,
) -> Result<()> {
    Ok(mctrin::instance::set_group_memberships(updates).await?)
}

#[tauri::command]
pub async fn instance_update_group_memberships(
    updates: Vec<InstanceGroupMembershipChange>,
) -> Result<()> {
    Ok(mctrin::instance::update_group_memberships(updates).await?)
}
