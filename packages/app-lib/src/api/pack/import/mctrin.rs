use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{
    State,
    install::{InstallPhaseDetails, InstallProgressReporter},
    state::{
        AppliedContentSetPatch, ContentSourceKind, EditInstance,
        InstanceInstallStage, InstanceLaunchOverridesPatch, ModLoader,
        ReleaseChannel, instances::InstanceLaunchOverridesData,
    },
    util::io,
};

use super::{ImportOverrides, finish_import, generic, recache_icon};

const CONFIG_FILE_NAME: &str = "mctrin_config.json";
/// Config file name written by releases before the MC-Trin rename; still read
/// so instances exported by older builds keep importing.
const LEGACY_CONFIG_FILE_NAME: &str = "axolotl_config.json";
const CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct MctrinConfigFile {
    pub schema_version: u32,
    pub instance_id: String,
    pub path: String,
    pub generated_at: chrono::DateTime<chrono::Utc>,
    pub name: String,
    pub icon_path: Option<String>,
    pub update_channel: String,
    pub symlink_target: Option<String>,
    pub groups: Vec<String>,
    pub content_set: MctrinContentSet,
    pub link: crate::state::instances::InstanceLink,
    pub launch_overrides: InstanceLaunchOverridesData,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct MctrinContentSet {
    pub source_kind: String,
    pub game_version: String,
    pub protocol_version: Option<u32>,
    pub loader: String,
    pub loader_version: Option<String>,
}

/// Finds the instance config to import: the current file name first, then the
/// pre-rename one, so instances exported by older builds keep importing.
async fn read_mctrin_config_path(source_path: &Path) -> Option<PathBuf> {
    let current = source_path.join(CONFIG_FILE_NAME);
    if tokio::fs::try_exists(&current).await.unwrap_or(false) {
        return Some(current);
    }
    let legacy = source_path.join(LEGACY_CONFIG_FILE_NAME);
    if tokio::fs::try_exists(&legacy).await.unwrap_or(false) {
        return Some(legacy);
    }
    None
}

/// Imports an MC-Trin instance by reading its `mctrin_config.json` (or the
/// pre-rename `axolotl_config.json`) and applying the migratable fields to
/// the new profile. Invalid or unsupported configs fall back to the generic
/// instance import so the game files still arrive.
pub(crate) async fn import_mctrin(
    source_path: PathBuf,
    instance_id: &str,
    reporter: InstallProgressReporter,
    details: InstallPhaseDetails,
    symlink: bool,
    overrides: &ImportOverrides,
) -> crate::Result<()> {
    let config_path = read_mctrin_config_path(&source_path)
        .await
        .unwrap_or_else(|| source_path.join(CONFIG_FILE_NAME));
    let content = io::read_any_encoding_to_string(&config_path)
        .await
        .map(|(content, _)| content)
        .unwrap_or_else(|error| {
            tracing::warn!(
                "MC-Trin import: could not read {}: {error}; falling back to generic import",
                config_path.display()
            );
            String::new()
        });

    let config = match serde_json::from_str::<MctrinConfigFile>(&content) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(
                "MC-Trin import: invalid config {}: {error}; falling back to generic import",
                config_path.display()
            );
            return generic::import_generic(
                source_path,
                instance_id.to_string(),
                reporter,
                details,
                symlink,
                overrides,
                None,
            )
            .await;
        }
    };
    tracing::debug!(
        "MC-Trin import: config instance_id={} path={} generated_at={} symlink_target={:?}",
        config.instance_id,
        config.path,
        config.generated_at,
        config.symlink_target
    );

    if config.schema_version != CONFIG_SCHEMA_VERSION
        || config.content_set.game_version.trim().is_empty()
    {
        tracing::warn!(
            "MC-Trin import: unsupported schema or missing game version for {}; falling back to generic import",
            config_path.display()
        );
        return generic::import_generic(
            source_path,
            instance_id.to_string(),
            reporter,
            details,
            symlink,
            overrides,
            None,
        )
        .await;
    }

    let icon = match config.icon_path.as_ref() {
        Some(path) => recache_icon(source_path.join(path)).await?,
        None => None,
    };
    let source_kind =
        ContentSourceKind::from_str(&config.content_set.source_kind)
            .unwrap_or(ContentSourceKind::Local);
    let loader = ModLoader::try_from_string(&config.content_set.loader)?;
    let update_channel = ReleaseChannel::from_key(&config.update_channel);

    let state = State::get().await?;
    crate::state::edit_instance(
        instance_id,
        EditInstance {
            install_stage: Some(InstanceInstallStage::PackInstalling),
            name: Some(config.name.clone()),
            icon_path: Some(icon.map(|p| p.to_string_lossy().to_string())),
            update_channel: Some(update_channel),
            groups: Some(config.groups.clone()),
            link: Some(config.link.clone()),
            content_set_patch: Some(AppliedContentSetPatch {
                source_kind: Some(source_kind),
                game_version: Some(config.content_set.game_version.clone()),
                protocol_version: Some(config.content_set.protocol_version),
                loader: Some(loader),
                loader_version: Some(config.content_set.loader_version.clone()),
            }),
            launch_overrides: Some(InstanceLaunchOverridesPatch {
                java_path: Some(config.launch_overrides.java_path.clone()),
                extra_launch_args: Some(
                    config.launch_overrides.extra_launch_args.clone(),
                ),
                custom_env_vars: Some(
                    config.launch_overrides.custom_env_vars.clone(),
                ),
                memory: Some(config.launch_overrides.memory),
                force_fullscreen: Some(
                    config.launch_overrides.force_fullscreen,
                ),
                maximize_window: Some(config.launch_overrides.maximize_window),
                game_resolution: Some(config.launch_overrides.game_resolution),
                window_title: Some(
                    config.launch_overrides.window_title.clone(),
                ),
                launch_preparation_timeout: Some(
                    config.launch_overrides.launch_preparation_timeout,
                ),
                hooks: Some(config.launch_overrides.hooks.clone()),
            }),
            ..EditInstance::default()
        },
        &state.pool,
    )
    .await?;

    finish_import(
        instance_id.to_string(),
        source_path,
        &state.io_semaphore,
        reporter,
        details,
        symlink,
    )
    .await
}
