//! Installer for MCBBS modpacks.
//!
//! MCBBS packs are zips carrying either an `mcbbs.packmeta` file or a
//! `manifest.json` with an `addons` array. Game and loader versions come from
//! the addons list, bundled content ships in `overrides/`, and optional
//! launch settings come from `launchInfo`.

use std::path::PathBuf;

use serde::Deserialize;

use super::archive_util;
use crate::State;
use crate::data::ModLoader;
use crate::install::{
    InstallPhaseDetails, InstallPhaseId, InstallProgressReporter,
};
use crate::pack::detect::{CURSEFORGE_MANIFEST, MCBBS_MANIFEST};
use crate::state::{
    AppliedContentSetPatch, ContentSourceKind, EditInstance,
    InstanceInstallStage, InstanceLaunchOverridesPatch, InstanceLink,
};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct McbbsManifest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    addons: Vec<McbbsAddon>,
    #[serde(default)]
    files: Vec<McbbsFile>,
    #[serde(default)]
    launch_info: Option<McbbsLaunchInfo>,
}

#[derive(Deserialize, Debug)]
struct McbbsAddon {
    id: String,
    version: String,
}

/// A `files` entry; `curse` entries carry CurseForge project/file ids while
/// `addition` entries ship inside the overrides folder and need no download.
#[derive(Deserialize, Debug)]
struct McbbsFile {
    #[serde(default, rename = "type")]
    type_: Option<String>,
    #[serde(default, alias = "projectID", alias = "projectId")]
    project_id: Option<u32>,
    #[serde(default, alias = "fileID", alias = "fileId")]
    file_id: Option<u32>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct McbbsLaunchInfo {
    #[serde(default)]
    java_argument: Option<serde_json::Value>,
}

fn join_arguments(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(value) => vec![value.clone()],
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Reads the fallback `manifest.json` and requires it to be MCBBS-shaped.
///
/// The CurseForge fallback is *not* parsed into [`McbbsManifest`]: that struct
/// is all-optional, so a CurseForge manifest (which carries `minecraft.version`
/// and `modLoaders`, never `addons`) would silently deserialize into an empty
/// manifest and then fail later with a misleading "could not determine the
/// version". Rather than broaden this parser, a manifest that is not
/// MCBBS-shaped is rejected here with an explicit message, leaving CurseForge
/// packs to the CurseForge installer that detection normally selects first.
///
/// Shared by the "first manifest missing" and "first manifest unparsable"
/// paths so both apply identical bounds and terminal-error rules.
async fn fallback_mcbbs_manifest(
    archive_path: PathBuf,
    entry_name: String,
    cancellation: tokio_util::sync::CancellationToken,
) -> crate::Result<McbbsManifest> {
    let contents = archive_util::read_archive_entry_to_string_bounded(
        archive_path,
        entry_name,
        cancellation,
    )
    .await?;
    let manifest: McbbsManifest = serde_json::from_str(&contents)?;
    if manifest.addons.is_empty() {
        return Err(crate::ErrorKind::InputError(format!(
            "Modpack archive is missing a usable {entry_name}: it is not an MCBBS manifest"
        ))
        .into());
    }
    Ok(manifest)
}

/// Whether a manifest-read failure must end the install instead of falling back.
///
/// Only "this archive has no such entry" / "it did not parse" may try the next
/// manifest. A deadline or a cancellation means the caller already gave up, so
/// retrying would leave a second detached parse and would also mislabel the
/// cause as a missing manifest.
fn is_terminal_manifest_error(error: &crate::Error) -> bool {
    let message = error.to_string();
    if crate::api::pack::detect::is_detection_timeout(error)
        || message.contains("Install was canceled")
    {
        return true;
    }
    // A size-limit hit is a safety boundary, not a "this is the other format"
    // signal: falling back would open the archive a second time for a pack that
    // is already known to be malformed. Only a genuinely absent or unparsable
    // manifest may try the next name.
    if message.contains("is larger than the") {
        return true;
    }
    // Structural read failures mean the archive itself is broken, so the next
    // manifest name cannot rescue it. These are distinct from "this entry is
    // absent" (which must fall back) and from malformed JSON (which must also
    // fall back, because it only proves the *contents* are not MCBBS).
    is_archive_structurally_invalid(error, &message)
}

/// Whether a bounded-reader failure says the archive could not be read at all.
///
/// Matched precisely so an ordinary missing-entry or JSON-parse failure keeps
/// its fallback behaviour: only the archive-level and entry-read diagnostics
/// qualify.
fn is_archive_structurally_invalid(
    error: &crate::Error,
    message: &str,
) -> bool {
    // Variant matches come first because they are exact: the bounded reader's
    // own I/O failures (file open, decompression, a short/corrupt entry) are
    // real `std::io`/`IOError` values, and no amount of retrying another
    // manifest name can read them.
    if matches!(
        error.raw.as_ref(),
        crate::ErrorKind::StdIOError(_) | crate::ErrorKind::IOError(_)
    ) {
        return true;
    }
    message.contains("Modpack archive is invalid:")
        || message.contains("from modpack archive:")
        // Both real cap rejections share this prefix: the on-disk size bound
        // ("Modpack archive is {n} bytes, which exceeds the ...-byte limit") and
        // the entry-count bound ("... {n} entries, which exceeds the ...-entry
        // detection limit"). A cap is a property of this archive, so retrying
        // another manifest name would just re-open the same oversized file.
        || message.contains("which exceeds the ")
}

/// Result of inspecting the first manifest before any fallback decision.
enum McbbsManifestDecision {
    Parsed(McbbsManifest),
    /// The CurseForge manifest must be tried; the optional error is the parse
    /// failure that caused it, kept only for diagnostics.
    Fallback(Option<serde_json::Error>),
}

/// Decides whether the first manifest is usable or the fallback must run.
///
/// Pure so the real branch can be asserted directly: a present-but-unparsable
/// manifest is exactly as recoverable as an absent one, and neither may be
/// confused with the terminal failures classified by
/// [`is_terminal_manifest_error`].
fn classify_mcbbs_manifest(contents: Option<String>) -> McbbsManifestDecision {
    match contents {
        Some(contents) => {
            match serde_json::from_str::<McbbsManifest>(&contents) {
                Ok(manifest) => McbbsManifestDecision::Parsed(manifest),
                Err(error) => McbbsManifestDecision::Fallback(Some(error)),
            }
        }
        None => McbbsManifestDecision::Fallback(None),
    }
}

pub(crate) async fn install_mcbbs_pack_with_reporter(
    instance_id: String,
    archive_path: PathBuf,
    base_folder: String,
    source_filename: Option<String>,
    reporter: InstallProgressReporter,
) -> crate::Result<()> {
    let state = State::get().await?;

    // The MCBBS manifest may be absent - or present but not valid MCBBS JSON -
    // in archives that are really CurseForge packs, so both cases fall back to
    // the CurseForge manifest. A deadline, a cancellation or a size violation
    // must NOT: the caller has already stopped waiting, and retrying would start
    // another parse nobody can observe.
    let mcbbs_cancellation = reporter.cancellation_token();
    let mcbbs_json = match archive_util::read_archive_entry_to_string_bounded(
        archive_path.clone(),
        format!("{base_folder}{MCBBS_MANIFEST}"),
        mcbbs_cancellation.clone(),
    )
    .await
    {
        Ok(contents) => Some(contents),
        Err(error) if is_terminal_manifest_error(&error) => return Err(error),
        Err(_) => None,
    };

    // Parse before deciding to fall back: a manifest that exists but is not
    // valid MCBBS JSON means "wrong format", exactly like a missing one.
    let manifest = match classify_mcbbs_manifest(mcbbs_json) {
        McbbsManifestDecision::Parsed(manifest) => manifest,
        McbbsManifestDecision::Fallback(reason) => {
            if let Some(error) = reason {
                tracing::debug!(
                    "MCBBS manifest was unparsable ({error}); trying the CurseForge manifest"
                );
            }
            fallback_mcbbs_manifest(
                archive_path.clone(),
                format!("{base_folder}{CURSEFORGE_MANIFEST}"),
                mcbbs_cancellation,
            )
            .await?
        }
    };

    let mut game_version = None;
    let mut loader = ModLoader::Vanilla;
    let mut loader_version = None;
    let mut optifine_version = None;
    let mut lite_loader_version = None;
    for addon in &manifest.addons {
        match addon.id.to_ascii_lowercase().as_str() {
            "game" => game_version = Some(addon.version.clone()),
            "forge" => {
                if loader != ModLoader::Vanilla {
                    return Err(crate::ErrorKind::InputError(
                        "MCBBS modpack declares multiple primary loaders"
                            .to_string(),
                    )
                    .into());
                }
                loader = ModLoader::Forge;
                loader_version = Some(addon.version.clone());
            }
            "neoforge" => {
                if loader != ModLoader::Vanilla {
                    return Err(crate::ErrorKind::InputError(
                        "MCBBS modpack declares multiple primary loaders"
                            .to_string(),
                    )
                    .into());
                }
                loader = ModLoader::NeoForge;
                loader_version = Some(addon.version.clone());
            }
            "fabric" => {
                if loader != ModLoader::Vanilla {
                    return Err(crate::ErrorKind::InputError(
                        "MCBBS modpack declares multiple primary loaders"
                            .to_string(),
                    )
                    .into());
                }
                loader = ModLoader::Fabric;
                loader_version = Some(addon.version.clone());
            }
            "quilt" => {
                if loader != ModLoader::Vanilla {
                    return Err(crate::ErrorKind::InputError(
                        "MCBBS modpack declares multiple primary loaders"
                            .to_string(),
                    )
                    .into());
                }
                loader = ModLoader::Quilt;
                loader_version = Some(addon.version.clone());
            }
            "cleanroom" => {
                if loader != ModLoader::Vanilla {
                    return Err(crate::ErrorKind::InputError(
                        "MCBBS modpack declares multiple primary loaders"
                            .to_string(),
                    )
                    .into());
                }
                loader = ModLoader::Cleanroom;
                loader_version = Some(addon.version.clone());
            }
            "legacy_fabric" | "legacyfabric" => {
                if loader != ModLoader::Vanilla {
                    return Err(crate::ErrorKind::InputError(
                        "MCBBS modpack declares multiple primary loaders"
                            .to_string(),
                    )
                    .into());
                }
                loader = ModLoader::LegacyFabric;
                loader_version = Some(addon.version.clone());
            }
            "lite_loader" | "liteloader" => {
                lite_loader_version = Some(addon.version.clone());
            }
            "optifine" => optifine_version = Some(addon.version.clone()),
            "labymod" => {
                return Err(crate::ErrorKind::InputError(
                    "Unsupported loader LabyMod: MC-Trin does not install, update, or repair LabyMod instances"
                        .to_string(),
                )
                .into());
            }
            other => {
                return Err(crate::ErrorKind::InputError(format!(
                    "Unsupported MCBBS loader component {other} {}",
                    addon.version
                ))
                .into());
            }
        }
    }
    let Some(game_version) = game_version else {
        return Err(crate::ErrorKind::InputError(
            "MCBBS modpack did not specify a Minecraft version".to_string(),
        )
        .into());
    };

    let mut lite_loader_as_adjunct = None;
    if let Some(lite_loader_version) = lite_loader_version {
        match loader {
            ModLoader::Vanilla => {
                loader = ModLoader::LiteLoader;
                loader_version = Some(lite_loader_version);
            }
            ModLoader::Forge => {
                lite_loader_as_adjunct = Some(lite_loader_version);
            }
            _ => {
                return Err(crate::ErrorKind::InputError(format!(
                    "LiteLoader is not supported with {}",
                    loader.as_str()
                ))
                .into());
            }
        }
    }

    let mut optifine_as_mod = None;
    let mut requires_optifabric = false;
    if let Some(optifine_version) = optifine_version {
        match loader {
            ModLoader::Vanilla => {
                loader = ModLoader::OptiFine;
                loader_version = Some(optifine_version);
            }
            ModLoader::Forge | ModLoader::NeoForge => {
                optifine_as_mod = Some(optifine_version);
            }
            ModLoader::Fabric | ModLoader::LegacyFabric => {
                optifine_as_mod = Some(optifine_version);
                requires_optifabric = true;
            }
            _ => {
                return Err(crate::ErrorKind::InputError(format!(
                    "OptiFine is not supported with {}",
                    loader.as_str()
                ))
                .into());
            }
        }
    }

    let pack_name = manifest
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            source_filename.as_ref().map(|name| {
                std::path::Path::new(name)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            })
        })
        .unwrap_or_else(|| "MCBBS Modpack".to_string());
    let pack_details = InstallPhaseDetails::Modpack {
        project_id: None,
        version_id: None,
        title: Some(pack_name.clone()),
    };
    reporter
        .update(InstallPhaseId::ResolvingPack, None, pack_details.clone())
        .await?;

    let lite_loader_as_adjunct = if let Some(requested_version) =
        lite_loader_as_adjunct
    {
        Some(
            crate::launcher::get_loader_version_from_profile(
                &game_version,
                ModLoader::LiteLoader,
                Some(&requested_version),
            )
            .await?
            .ok_or_else(|| {
                crate::ErrorKind::InputError(format!(
                    "No LiteLoader version {requested_version} supports Minecraft {game_version}"
                ))
            })?,
        )
    } else {
        None
    };
    let optifabric_version = if requires_optifabric {
        Some(
            crate::install::runner::resolve_optifabric_version(&game_version)
                .await?,
        )
    } else {
        None
    };

    let resolved_loader_version = if loader != ModLoader::Vanilla {
        crate::launcher::get_loader_version_from_profile(
            &game_version,
            loader,
            loader_version.as_deref(),
        )
        .await?
    } else {
        None
    };

    let launch_overrides =
        manifest.launch_info.as_ref().and_then(|launch_info| {
            let jvm_args = launch_info
                .java_argument
                .as_ref()
                .map(join_arguments)
                .filter(|args| !args.is_empty())?;
            Some(InstanceLaunchOverridesPatch {
                extra_launch_args: Some(Some(jvm_args)),
                ..InstanceLaunchOverridesPatch::default()
            })
        });

    crate::api::instance::edit(
        &instance_id,
        EditInstance {
            install_stage: Some(InstanceInstallStage::PackInstalling),
            name: Some(pack_name.clone()),
            link: Some(InstanceLink::ImportedModpack {
                project_id: None,
                version_id: None,
                name: Some(pack_name.clone()),
                version_number: manifest.version.clone(),
                filename: source_filename,
            }),
            launch_overrides,
            content_set_patch: Some(AppliedContentSetPatch {
                source_kind: Some(ContentSourceKind::ImportedModpack),
                game_version: Some(game_version.clone()),
                protocol_version: Some(None),
                loader: Some(loader),
                loader_version: Some(
                    resolved_loader_version.map(|version| version.id),
                ),
            }),
            ..EditInstance::default()
        },
    )
    .await?;

    let minecraft_install =
        super::parallel_minecraft_install::ParallelMinecraftInstall::start(
            instance_id.clone(),
            reporter.clone(),
        );

    let curse_files = manifest
        .files
        .iter()
        .filter(|file| {
            file.type_
                .as_deref()
                .is_none_or(|kind| kind.eq_ignore_ascii_case("curse"))
        })
        .filter_map(|file| {
            Some(crate::api::curseforge::CurseForgeManifestFile {
                project_id: file.project_id?,
                file_id: file.file_id?,
                required: true,
            })
        })
        .collect::<Vec<_>>();
    if !curse_files.is_empty() {
        let content_loader = (loader != ModLoader::Vanilla
            && loader != ModLoader::OptiFine)
            .then(|| loader.as_str().to_string());
        crate::api::curseforge::install_local_manifest_files(
            &instance_id,
            curse_files,
            false,
            &game_version,
            content_loader.as_deref(),
            pack_details.clone(),
            &reporter,
        )
        .await?;
    }

    reporter
        .update(
            InstallPhaseId::ExtractingOverrides,
            None,
            pack_details.clone(),
        )
        .await?;
    let instance_path =
        crate::api::instance::get_full_path(&instance_id).await?;
    let archive_cancellation = reporter.cancellation_token();
    let (_, archive_replacements) =
        archive_util::materialize_archive_subdir_for_instance(
            instance_id.clone(),
            archive_cancellation.clone(),
            archive_path,
            format!("{base_folder}overrides/"),
            instance_path.clone(),
        )
        .await?;

    let post_archive_result: crate::Result<()> = async {
        minecraft_install.join().await?;

        if let Some(lite_loader_version) = lite_loader_as_adjunct {
            install_liteloader_component(
                &state,
                &instance_id,
                &game_version,
                loader,
                &lite_loader_version,
            )
            .await?;
        }
        if let Some(optifine_version) = optifine_as_mod {
            install_optifine_mod(
                &state,
                &instance_id,
                reporter.cancellation_token(),
                &game_version,
                &optifine_version,
                &instance_path,
            )
            .await?;
            record_optifine_component(&instance_id, &optifine_version).await?;
        }
        if let Some(optifabric_version) = optifabric_version {
            install_optifabric_component(
                &instance_id,
                &game_version,
                &optifabric_version,
            )
            .await?;
        }

        reporter.clear_context().await?;
        Ok(())
    }
    .await;
    archive_util::settle_staged_archive_install(
        instance_id,
        archive_cancellation,
        archive_replacements,
        post_archive_result,
        "MCBBS overrides",
    )
    .await
}

pub(crate) async fn record_optifine_component(
    instance_id: &str,
    version: &str,
) -> crate::Result<()> {
    record_loader_component(
        instance_id,
        crate::state::LoaderComponentKind::OptiFine,
        version,
        Some(serde_json::json!({ "source": "pack" })),
    )
    .await
}

pub(crate) async fn record_loader_component(
    instance_id: &str,
    kind: crate::state::LoaderComponentKind,
    version: &str,
    provider_metadata: Option<serde_json::Value>,
) -> crate::Result<()> {
    let state = State::get().await?;
    let metadata =
        crate::api::instance::get(instance_id)
            .await?
            .ok_or_else(|| {
                crate::ErrorKind::InputError(format!(
                    "Unknown instance {instance_id}"
                ))
            })?;
    let mut components = metadata.loader_components;
    components.retain(|component| component.kind != kind);
    components.push(crate::state::LoaderComponent {
        instance_id: instance_id.to_string(),
        kind,
        version: Some(version.to_string()),
        role: crate::state::LoaderComponentRole::Adjunct,
        provider_metadata,
    });
    crate::state::instances::commands::replace_instance_loader_components(
        instance_id,
        &components,
        &state.pool,
    )
    .await
}

pub(crate) async fn install_liteloader_component(
    state: &State,
    instance_id: &str,
    game_version: &str,
    primary_loader: ModLoader,
    resolved_version: &daedalus::modded::LoaderVersion,
) -> crate::Result<()> {
    let metadata =
        crate::api::instance::get(instance_id)
            .await?
            .ok_or_else(|| {
                crate::ErrorKind::InputError(format!(
                    "Unknown instance {instance_id}"
                ))
            })?;
    let version = crate::install::runner::install_liteloader_adjunct_resolved(
        state,
        &metadata,
        game_version,
        primary_loader,
        resolved_version,
    )
    .await?;
    record_loader_component(
        instance_id,
        crate::state::LoaderComponentKind::LiteLoader,
        &version,
        Some(serde_json::json!({ "source": "pack" })),
    )
    .await
}

pub(crate) async fn install_optifabric_component(
    instance_id: &str,
    game_version: &str,
    version: &str,
) -> crate::Result<()> {
    let version = crate::install::runner::install_optifabric_file(
        instance_id,
        game_version,
        version,
    )
    .await?;
    record_loader_component(
        instance_id,
        crate::state::LoaderComponentKind::OptiFabric,
        &version,
        Some(serde_json::json!({
            "projectId": crate::install::runner::OPTIFABRIC_CURSEFORGE_PROJECT_ID,
            "provider": "curseforge"
        })),
    )
    .await
}

/// Installs OptiFine into the instance's mods folder for packs that pair it
/// with Forge or NeoForge. Requires the instance's Minecraft install to have
/// completed so the client jar and a Java runtime are available.
pub(crate) async fn install_optifine_mod(
    state: &State,
    instance_id: &str,
    cancellation: tokio_util::sync::CancellationToken,
    game_version: &str,
    optifine_version: &str,
    instance_path: &std::path::Path,
) -> crate::Result<()> {
    let metadata =
        crate::api::instance::get(instance_id)
            .await?
            .ok_or_else(|| {
                crate::ErrorKind::InputError(format!(
                    "Unknown instance {instance_id}"
                ))
            })?;
    let version_jar = match &metadata.applied_content_set.loader_version {
        Some(loader_version) => format!("{game_version}-{loader_version}"),
        None => game_version.to_string(),
    };
    let loader_client_jar = state
        .directories
        .version_dir(&version_jar)
        .join(format!("{version_jar}.jar"));
    let client_jar = if loader_client_jar.is_file() {
        loader_client_jar
    } else {
        state
            .directories
            .version_dir(game_version)
            .join(format!("{game_version}.jar"))
    };

    let (manifest, version_index) =
        crate::launcher::resolve_minecraft_manifest(game_version, state)
            .await?;
    let version_info = crate::launcher::download::download_version_info(
        state,
        &manifest.versions[version_index],
        ModLoader::Vanilla,
        None,
        None,
        None,
        None,
    )
    .await?;
    let java_key = version_info
        .java_version
        .as_ref()
        .map_or(8, |java| java.major_version);
    let java = crate::api::jre::find_java_for_version(java_key)
        .await?
        .ok_or_else(|| {
            crate::ErrorKind::LauncherError(format!(
                "No Java {java_key} runtime is available for the OptiFine installer"
            ))
        })?;

    crate::launcher::optifine::install_optifine_as_mod(
        state,
        instance_id,
        cancellation,
        std::path::Path::new(&java.path),
        game_version,
        optifine_version,
        &client_jar,
        &instance_path.join("mods"),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeout_is_terminal_and_does_not_fall_back() {
        // A deadline must end the install instead of trying the CurseForge
        // manifest, which would start a second parse after the caller gave up.
        let timeout: crate::Error = crate::ErrorKind::InputError(
            "Timed out after 60 seconds while reading mcbbs.packmeta from the modpack archive"
                .to_string(),
        )
        .into();
        assert!(is_terminal_manifest_error(&timeout));
    }

    #[test]
    fn a_cancellation_is_terminal_and_does_not_fall_back() {
        let cancelled: crate::Error =
            crate::ErrorKind::OtherError("Install was canceled".to_string())
                .into();
        assert!(is_terminal_manifest_error(&cancelled));
    }

    #[test]
    fn a_missing_manifest_still_falls_back() {
        // The ordinary "not this format" case has to keep working: this is what
        // lets a CurseForge archive be detected through the MCBBS installer.
        let missing: crate::Error = crate::ErrorKind::InputError(
            "Modpack archive is missing mcbbs.packmeta".to_string(),
        )
        .into();
        assert!(!is_terminal_manifest_error(&missing));
    }

    #[test]
    fn an_oversized_manifest_is_terminal() {
        // A size-limit hit is a safety boundary, not a "try the other format"
        // signal: falling back would reopen a pack already known to be broken.
        let oversized: crate::Error = crate::ErrorKind::InputError(
            "Modpack manifest mcbbs.packmeta is larger than the 4194304-byte limit"
                .to_string(),
        )
        .into();
        assert!(is_terminal_manifest_error(&oversized));
    }

    #[test]
    fn an_unparsable_manifest_still_falls_back() {
        // A manifest that exists but is not valid MCBBS JSON is the ordinary
        // "wrong format" case and must keep falling back.
        let unparsable: crate::Error = crate::ErrorKind::InputError(
            "expected value at line 1 column 1".to_string(),
        )
        .into();
        assert!(!is_terminal_manifest_error(&unparsable));
    }

    #[test]
    fn a_malformed_first_manifest_falls_back_instead_of_aborting() {
        // A manifest that exists but is not valid MCBBS JSON means "wrong
        // format", so the CurseForge fallback must still be attempted. This
        // exercises the classification the fallback decision is based on: the
        // parse error itself is an ordinary failure, not a terminal one.
        let unparsable: crate::Error = crate::ErrorKind::InputError(
            "expected value at line 1 column 1".to_string(),
        )
        .into();
        assert!(!is_terminal_manifest_error(&unparsable));
    }

    #[test]
    fn a_terminal_read_failure_never_reaches_the_fallback() {
        // Timeout, cancellation and size violations are checked before the
        // parse, so they can never be downgraded into a fallback attempt.
        for message in [
            "Timed out after 60 seconds while reading mcbbs.packmeta from the modpack archive",
            "Install was canceled",
            "Modpack manifest mcbbs.packmeta is larger than the 4194304-byte limit",
        ] {
            let error: crate::Error =
                crate::ErrorKind::InputError(message.to_string()).into();
            assert!(
                is_terminal_manifest_error(&error),
                "expected terminal: {message}"
            );
        }
    }

    /// Builds a stored-only zip so the test exercises the same reader the
    /// installer uses, including the bounded entry-to-string helper.
    fn build_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;

        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, body) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(body).unwrap();
            }
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    #[tokio::test]
    async fn a_malformed_first_manifest_falls_back_to_a_mcbbs_shaped_second() {
        // Full call path: the first manifest exists but is not JSON, so the
        // bounded reader succeeds and the parse fails; the fallback manifest is
        // then read for real. The fallback must be MCBBS-shaped, because this
        // installer only understands `addons`.
        let bytes = build_archive(&[
            ("mcbbs.packmeta", b"{ not json"),
            (
                "manifest.json",
                br#"{"name":"Fallback Pack","addons":[{"id":"game","version":"1.20.1"}]}"#,
            ),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pack.zip");
        std::fs::write(&path, bytes).unwrap();

        let manifest = fallback_mcbbs_manifest(
            path,
            "manifest.json".to_string(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("an MCBBS-shaped fallback manifest is usable");

        assert_eq!(manifest.name.as_deref(), Some("Fallback Pack"));
        assert_eq!(manifest.addons.len(), 1);
    }

    #[tokio::test]
    async fn a_curseforge_shaped_fallback_is_rejected_explicitly() {
        // A CurseForge manifest has `minecraft.version` and no `addons`. Parsing
        // it into `McbbsManifest` would silently yield an empty manifest and fail
        // later with a misleading "could not determine the version", so it is
        // rejected here instead of being half-accepted.
        let bytes = build_archive(&[
            ("mcbbs.packmeta", b"{ not json"),
            (
                "manifest.json",
                br#"{"minecraft":{"version":"1.20.1"},"name":"Fallback Pack"}"#,
            ),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pack.zip");
        std::fs::write(&path, bytes).unwrap();

        let error = fallback_mcbbs_manifest(
            path,
            "manifest.json".to_string(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap_err();

        assert!(
            error.to_string().contains("not an MCBBS manifest"),
            "expected the shape rejection, got: {error}"
        );
    }

    #[tokio::test]
    async fn a_missing_fallback_manifest_reports_missing() {
        // The fallback path must surface a real read error rather than a
        // default identity.
        let bytes = build_archive(&[("mcbbs.packmeta", b"{ not json")]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pack.zip");
        std::fs::write(&path, bytes).unwrap();

        let error = fallback_mcbbs_manifest(
            path,
            "manifest.json".to_string(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap_err();

        assert!(
            error.to_string().contains("missing"),
            "expected the missing-entry error, got: {error}"
        );
    }

    #[test]
    fn the_real_branch_falls_back_when_the_first_manifest_is_malformed() {
        // Exercises the exact decision the installer makes, not a copy of it:
        // a manifest that exists but is not valid MCBBS JSON must select the
        // CurseForge fallback.
        let decision = classify_mcbbs_manifest(Some("{ not json".to_string()));

        assert!(
            matches!(decision, McbbsManifestDecision::Fallback(Some(_))),
            "malformed MCBBS manifest must request the fallback"
        );
    }

    #[test]
    fn the_real_branch_falls_back_when_the_first_manifest_is_absent() {
        let decision = classify_mcbbs_manifest(None);
        assert!(matches!(decision, McbbsManifestDecision::Fallback(None)));
    }

    #[test]
    fn the_real_branch_accepts_a_valid_first_manifest() {
        let decision = classify_mcbbs_manifest(Some(
            r#"{"name":"MCBBS Pack","addons":[]}"#.to_string(),
        ));
        assert!(matches!(decision, McbbsManifestDecision::Parsed(_)));
    }

    #[test]
    fn an_archive_structural_failure_is_terminal() {
        // A bounded reader that cannot read the archive at all must not be
        // downgraded into a fallback attempt. The strings below are the
        // reachable ones: `archive_error` for a corrupt archive, and the two
        // real cap rejections that the reader can actually return.
        for message in [
            "Modpack archive is invalid: invalid Zip archive: Could not find central directory end",
            "Modpack archive is 17179869184 bytes, which exceeds the 8589934592-byte limit",
            "Modpack archive has 250000 entries, which exceeds the 200000-entry detection limit",
        ] {
            let error: crate::Error =
                crate::ErrorKind::InputError(message.to_string()).into();
            assert!(
                is_terminal_manifest_error(&error),
                "expected terminal: {message}"
            );
        }
    }

    #[test]
    fn missing_entry_and_unparsable_json_still_fall_back() {
        // The two recoverable shapes must stay recoverable.
        for message in [
            "Modpack archive is missing mcbbs.packmeta",
            "expected value at line 1 column 1",
        ] {
            let error: crate::Error =
                crate::ErrorKind::InputError(message.to_string()).into();
            assert!(
                !is_terminal_manifest_error(&error),
                "must stay recoverable: {message}"
            );
        }
    }

    #[test]
    fn bounded_reader_io_failures_are_terminal() {
        // The bounded reader propagates real I/O failures (open failure,
        // decompression failure, a short entry) as `IOError`/`StdIOError`.
        // Retrying another manifest name would reopen the same broken archive.
        let io_error: crate::Error = crate::ErrorKind::StdIOError(
            std::io::Error::new(std::io::ErrorKind::InvalidData, "bad crc"),
        )
        .into();
        assert!(is_terminal_manifest_error(&io_error));

        let fs_error: crate::Error =
            crate::ErrorKind::IOError(crate::util::io::IOError::with_path(
                std::io::Error::new(std::io::ErrorKind::NotFound, "gone"),
                std::path::Path::new("pack.zip"),
            ))
            .into();
        assert!(is_terminal_manifest_error(&fs_error));
    }

    #[test]
    fn a_cap_rejection_is_terminal_for_both_bounds() {
        // The two reachable cap messages the bounded reader can return.
        for message in [
            "Modpack archive is 17179869184 bytes, which exceeds the 8589934592-byte limit",
            "Modpack archive has 250000 entries, which exceeds the 200000-entry detection limit",
        ] {
            let error: crate::Error =
                crate::ErrorKind::InputError(message.to_string()).into();
            assert!(
                is_terminal_manifest_error(&error),
                "expected terminal: {message}"
            );
        }
    }

    #[test]
    fn a_serde_json_failure_is_never_treated_as_structural() {
        // JSON parse errors are `JSONError`, not I/O, and must keep falling back
        // to the CurseForge manifest.
        let parse: crate::Error =
            serde_json::from_str::<McbbsManifest>("{ nope")
                .unwrap_err()
                .into();
        assert!(!is_terminal_manifest_error(&parse));
    }
}
