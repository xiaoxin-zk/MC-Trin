use crate::State;
use crate::data::ModLoader;
use crate::install::{
    InstallErrorContext, InstallJobEventKind, InstallPhaseDetails,
    InstallPhaseId, InstallProgress, InstallProgressReporter,
};
use crate::state::{
    AppliedContentSetPatch, CacheBehaviour, CachedEntry, ContentSourceKind,
    EditInstance, InstanceInstallStage, InstanceLink, LoaderComponent,
    LoaderComponentKind, LoaderComponentRole, ModrinthProjectId,
    ModrinthVersionId, SideType,
};
use crate::util::download::constants::{
    INSTALL_FILE_REPORTED_ATTEMPTS, MODPACK_RANGE_CONNECTION_FLOOR,
    MODPACK_REQUESTED_H2_RANGES,
};
use crate::util::fetch::{
    ContentValidation, DownloadMeta, DownloadReason, DownloadRequest,
    FetchProgressFn, Integrity, ResourceClass,
    download_to_path_without_executor, fetch_with_pool, write_cached_icon,
};
use path_util::SafeRelativeUtf8UnixPathBuf;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;

use std::path::{Path, PathBuf};
use std::pin::Pin;

#[derive(Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PackFormat {
    pub game: String,
    pub format_version: i32,
    pub version_id: String,
    pub name: String,
    pub summary: Option<String>,
    pub files: Vec<PackFile>,
    pub dependencies: HashMap<PackDependency, String>,
}

#[derive(Serialize, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PackFile {
    pub path: SafeRelativeUtf8UnixPathBuf,
    pub hashes: HashMap<PackFileHash, String>,
    pub env: Option<HashMap<EnvType, SideType>>,
    pub downloads: Vec<String>,
    pub file_size: u64,
}

#[derive(Serialize, Deserialize, Clone, Eq, PartialEq, Hash)]
#[serde(rename_all = "camelCase", from = "String")]
pub enum PackFileHash {
    Sha1,
    Sha512,
    Unknown(String),
}

impl From<String> for PackFileHash {
    fn from(s: String) -> Self {
        match s.as_str() {
            "sha1" => PackFileHash::Sha1,
            "sha512" => PackFileHash::Sha512,
            _ => PackFileHash::Unknown(s),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Eq, PartialEq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum EnvType {
    Client,
    Server,
}

#[derive(Serialize, Deserialize, Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub enum PackDependency {
    #[serde(rename = "forge")]
    Forge,

    #[serde(rename = "neoforge")]
    #[serde(alias = "neo-forge")]
    NeoForge,

    #[serde(rename = "fabric-loader")]
    FabricLoader,

    #[serde(rename = "quilt-loader")]
    QuiltLoader,

    #[serde(rename = "optifine")]
    OptiFine,

    #[serde(rename = "cleanroom")]
    Cleanroom,

    #[serde(rename = "lite_loader", alias = "liteloader")]
    LiteLoader,

    #[serde(rename = "legacy_fabric", alias = "legacyfabric")]
    LegacyFabric,

    #[serde(rename = "optifabric")]
    OptiFabric,

    #[serde(rename = "minecraft")]
    Minecraft,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum CreatePackLocation {
    // Create a pack from a modrinth version ID (such as a modpack)
    FromVersionId {
        project_id: String,
        version_id: String,
        title: String,
        icon_url: Option<String>,
    },
    // Create a pack from a file (such as an .mrpack for installing from a file, or a folder name for importing)
    FromFile {
        path: PathBuf,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePackInstance {
    pub name: String, // the name of the instance and relative path
    pub game_version: String, // the game version of the instance
    pub modloader: ModLoader, // the modloader to use
    pub loader_version: Option<String>, // the modloader version to use, set to "latest", "stable", or the ID of your chosen loader. defaults to latest
    pub icon: Option<PathBuf>,          // the icon for the instance
    pub icon_url: Option<String>, // the URL icon for an instance during import
    pub link: Option<InstanceLink>,
    pub unknown_file: bool, // true when pack file isn't found on Modrinth via hash lookup
    pub skip_install_profile: Option<bool>,
    pub no_watch: Option<bool>,
}

// default
impl Default for CreatePackInstance {
    fn default() -> Self {
        CreatePackInstance {
            name: "Untitled".to_string(),
            game_version: "1.19.4".to_string(),
            modloader: ModLoader::Vanilla,
            loader_version: None,
            icon: None,
            icon_url: None,
            link: None,
            unknown_file: false,
            skip_install_profile: Some(true),
            no_watch: Some(false),
        }
    }
}

#[derive(Clone)]
pub enum CreatePackFile {
    Bytes(bytes::Bytes),
    // Local packs can be larger than available memory, so keep them file-backed.
    Path(PathBuf),
}

#[derive(Clone)]
pub struct CreatePack {
    pub file: CreatePackFile,
    pub description: CreatePackDescription,
}

#[derive(Clone, Debug)]
pub struct CreatePackDescription {
    pub icon: Option<PathBuf>,
    pub override_title: Option<String>,
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub instance_id: String,
    pub source_filename: Option<String>,
}

/// Opens a local pack's central directory on the blocking pool, with a deadline
/// and cooperative cancellation.
///
/// Kept separate from format detection because the two answer different
/// questions and map errors differently: this one must surface a corrupt
/// archive as a terminal `Invalid or corrupt modpack archive` error, while
/// detection treats an unreadable archive as merely unrecognised. Both are
/// bounded, because both walk the same central directory.
///
/// The bound is cooperative, not preemptive. `ZipArchive::new` is one
/// synchronous call that cannot be interrupted, so cancellation is only
/// observable after it returns; a stalled disk can hold this worker past the
/// deadline. The token plus RAII guard ensure no *second* detached parser is
/// left running with nobody able to signal it, which is what the caller's
/// timeout alone could not do.
async fn validate_local_pack_archive_openable(
    path: &std::path::Path,
) -> crate::Result<()> {
    use crate::api::pack::detect::DETECT_TIMEOUT;
    let owned = path.to_path_buf();
    let display = path.display().to_string();
    let worker_cancellation = tokio_util::sync::CancellationToken::new();
    let mut guard = OpenProbeGuard::new(worker_cancellation.clone());
    let worker = tokio::task::spawn_blocking(move || {
        // Inside the worker so the caller's deadline also covers the metadata
        // stat: `std::fs::metadata` blocks on network paths like any other
        // filesystem call, and doing it before the spawn would park the runtime
        // outside the timeout.
        crate::api::pack::detect::check_archive_size(&owned)?;
        let file = std::fs::File::open(&owned)
            .map_err(crate::ErrorKind::StdIOError)?;
        let archive = zip::ZipArchive::new(file).map_err(|e| {
            crate::ErrorKind::InputError(format!(
                "Invalid or corrupt modpack archive ({display}): {e}"
            ))
        })?;
        // First cancellation point: the constructor above cannot be
        // interrupted, so this is the earliest the worker can obey the flag.
        if worker_cancellation.is_cancelled() {
            return Err(open_probe_cancelled());
        }
        drop(archive);
        Ok(())
    });

    match tokio::time::timeout(DETECT_TIMEOUT, worker).await {
        Ok(result) => {
            guard.disarm();
            result?
        }
        Err(_) => {
            guard.cancel();
            Err(crate::ErrorKind::InputError(format!(
                "Timed out after {} seconds while reading the local modpack archive",
                DETECT_TIMEOUT.as_secs()
            ))
            .into())
        }
    }
}

fn open_probe_cancelled() -> crate::Error {
    crate::ErrorKind::OtherError("Install was canceled".to_string()).into()
}

/// Cancels an open-probe worker when the awaiting future is dropped.
///
/// Without this, a timeout only drops the join handle: the blocking task keeps
/// running with no signal at all, which is the same detached-parser problem the
/// detection path had.
struct OpenProbeGuard {
    token: tokio_util::sync::CancellationToken,
    active: bool,
}

impl OpenProbeGuard {
    fn new(token: tokio_util::sync::CancellationToken) -> Self {
        Self {
            token,
            active: true,
        }
    }

    fn disarm(&mut self) {
        self.active = false;
    }

    fn cancel(&mut self) {
        self.token.cancel();
        self.active = false;
    }
}

impl Drop for OpenProbeGuard {
    fn drop(&mut self) {
        if self.active {
            self.token.cancel();
        }
    }
}

pub async fn get_instance_from_pack(
    location: CreatePackLocation,
) -> crate::Result<CreatePackInstance> {
    match location {
        CreatePackLocation::FromVersionId {
            project_id,
            version_id,
            title,
            icon_url,
        } => Ok(CreatePackInstance {
            name: title,
            icon_url,
            link: Some(InstanceLink::ModrinthModpack {
                project_id,
                version_id,
            }),
            ..Default::default()
        }),
        CreatePackLocation::FromFile { path } => {
            let file_name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            // Validate ZIP structure before proceeding — fail fast on corrupt
            // archives rather than discovering the error later during
            // extraction. Bounded like every other central-directory read:
            // this used to be a bare `spawn_blocking(...).await??`, which left
            // the request waiting on an uninterruptible parse with no deadline.
            validate_local_pack_archive_openable(&path).await?;

            // Detect first, because CurseForge packs may be wrapped in one
            // top-level folder: the probe below needs the detected base prefix
            // or it would report a valid wrapped pack as missing its manifest.
            let detected =
                match crate::api::pack::detect::detect_local_pack_with_timeout(
                    &path,
                )
                .await
                {
                    Ok(detected) => Some(detected),
                    Err(error)
                        if crate::api::pack::detect::is_detection_timeout(
                            &error,
                        ) =>
                    {
                        return Err(error);
                    }
                    Err(_) => None,
                };

            // CurseForge archives carry authoritative identity metadata in
            // manifest.json. Read it before instance creation so we do not
            // briefly create a 1.19.4/Vanilla instance. `base` keeps the probe
            // aware of a wrapper folder while still preferring the archive root.
            let detected_base = detected
                .as_ref()
                .map(|detected| detected.base_folder.as_str())
                .unwrap_or("");
            match crate::api::curseforge::get_local_modpack_target_at(
                &path,
                detected_base,
            )
            .await
            {
                Ok((pack_name, target)) => {
                    return Ok(CreatePackInstance {
                        name: pack_name,
                        game_version: target.game_version,
                        modloader: target.loader,
                        loader_version: target.loader_version,
                        unknown_file: false,
                        ..Default::default()
                    });
                }
                Err(error)
                    if crate::api::pack::detect::is_detection_timeout(
                        &error,
                    ) =>
                {
                    return Err(error);
                }
                Err(_) => {}
            }

            let is_known_file = detected.is_some();

            Ok(CreatePackInstance {
                name: file_name,
                unknown_file: !is_known_file,
                ..Default::default()
            })
        }
    }
}

#[tracing::instrument(skip(reporter))]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn generate_pack_from_version_id_with_reporter(
    project_id: String,
    version_id: String,
    title: String,
    icon_url: Option<String>,
    instance_id: String,
    reason: DownloadReason,
    reporter: InstallProgressReporter,
) -> crate::Result<CreatePack> {
    let state = State::get().await?;
    let has_icon_url = icon_url.is_some();
    let cancellation = reporter.cancellation_token();
    let version_id_ref = ModrinthVersionId::new(version_id.clone())?;
    let version_query = CachedEntry::get_version(
        &version_id_ref,
        Some(CacheBehaviour::Bypass),
        &state.pool,
        &state.api_semaphore,
    );
    let version = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(crate::resources::protocol::error(crate::resources::ResourceError::Canceled)),
        version = version_query => version?,
    }
    .ok_or_else(|| {
        crate::ErrorKind::InputError(
            "Invalid version ID specified!".to_string(),
        )
    })?;

    if version.project_id != project_id {
        return Err(crate::ErrorKind::InputError(
            "The selected modpack release belongs to a different project"
                .into(),
        )
        .into());
    }

    // Update instance with correct loader and game version from the API version metadata,
    // so the UI shows accurate info while the pack file is still downloading.
    if let Some(game_version) = version.game_versions.first() {
        let loader = version
            .loaders
            .first()
            .map(|loader| ModLoader::try_from_string(loader))
            .transpose()?
            .unwrap_or(ModLoader::Vanilla);
        let game_version = game_version.clone();
        crate::api::instance::edit(
            &instance_id,
            EditInstance {
                content_set_patch: Some(AppliedContentSetPatch {
                    source_kind: None,
                    game_version: Some(game_version),
                    protocol_version: Some(None),
                    loader: Some(loader),
                    loader_version: None,
                }),
                ..EditInstance::default()
            },
        )
        .await?;
    }

    let pack_file = version
        .files
        .iter()
        .find(|file| file.primary)
        .or_else(|| version.files.first())
        .ok_or_else(|| {
            crate::ErrorKind::InputError(
                "Specified version has no files".to_string(),
            )
        })?;
    let hash = pack_file.hashes.get("sha1");
    let pack_path = modpack_cache_path(
        &state.directories.caches_dir().join("modpacks"),
        &project_id,
        &version_id,
        &pack_file.filename,
    )?;

    let metadata =
        crate::api::instance::get(&instance_id)
            .await?
            .ok_or_else(|| {
                crate::ErrorKind::InputError(format!(
                    "Unknown instance {instance_id}"
                ))
            })?;

    let download_meta = DownloadMeta {
        reason,
        game_version: metadata.applied_content_set.game_version.clone(),
        loader: metadata.applied_content_set.loader.as_str().to_string(),
        dependent_on: Some(version_id.clone()),
    };

    let details = InstallPhaseDetails::Modpack {
        project_id: Some(project_id.clone()),
        version_id: Some(version_id.clone()),
        title: Some(title.clone()),
    };
    let mut last_reported_bytes = 0_u64;
    let mut progress =
        |current: u64,
         total: u64|
         -> Pin<Box<dyn Future<Output = crate::Result<()>> + Send>> {
            let min_delta = (total / 200).max(256 * 1024);
            if current < total
                && current.saturating_sub(last_reported_bytes) < min_delta
            {
                return Box::pin(async { Ok(()) });
            }

            last_reported_bytes = current;
            let reporter = reporter.clone();
            let details = details.clone();
            Box::pin(async move {
                reporter
                    .update(
                        InstallPhaseId::DownloadingPackFile,
                        Some(InstallProgress {
                            current,
                            total,
                            secondary: None,
                        }),
                        details,
                    )
                    .await?;
                Ok(())
            })
        };
    let progress = Some(&mut progress as &mut FetchProgressFn<'_>);

    let context = InstallErrorContext::new("download modpack file")
        .urls(vec![pack_file.url.clone()])
        .maybe_expected_hash(hash.cloned())
        .expected_size(pack_file.size as u64)
        .target_path(pack_path.display().to_string())
        .project_id(project_id.clone())
        .version_id(version_id.clone())
        .build();
    reporter.set_context(context).await?;
    let item_path = pack_path.display().to_string();
    reporter
        .update_with_events(
            InstallPhaseId::DownloadingPackFile,
            Some(InstallProgress {
                current: 0,
                total: pack_file.size.max(1) as u64,
                secondary: None,
            }),
            details.clone(),
            vec![InstallJobEventKind::ContentFileQueued {
                path: item_path,
                bytes_total: Some(pack_file.size as u64),
                max_attempts: INSTALL_FILE_REPORTED_ATTEMPTS,
            }],
        )
        .await?;
    reporter.persist().await?;
    let download_result = download_to_path_without_executor(
        DownloadRequest::new(&pack_file.url, ResourceClass::Modpack)
            .with_integrity(Integrity {
                size: Some(pack_file.size as u64),
                sha1: hash.cloned(),
                sha512: pack_file.hashes.get("sha512").cloned(),
                content: ContentValidation::Jar,
                ..Integrity::default()
            })
            .with_h2_range_concurrency(MODPACK_REQUESTED_H2_RANGES)
            .with_range_connection_floor(MODPACK_RANGE_CONNECTION_FLOOR)
            .with_download_meta(download_meta)
            .with_install_tracking(
                reporter.clone(),
                pack_path.display().to_string(),
                pack_file.filename.clone(),
            ),
        &pack_path,
        &state.download_semaphore,
        progress,
    )
    .await?;
    if download_result.attempts > 0 {
        reporter
            .record_download_metrics(
                download_result.source.as_str(),
                download_result.fallback_count as u64,
            )
            .await?;
    }

    reporter
        .update(InstallPhaseId::ResolvingPack, None, details.clone())
        .await?;

    let project = CachedEntry::get_project(
        &ModrinthProjectId::new(version.project_id.clone())?,
        None,
        &state.pool,
        &state.api_semaphore,
    )
    .await?
    .ok_or_else(|| {
        crate::ErrorKind::InputError(
            "Invalid project ID specified!".to_string(),
        )
    })?;

    // Only fetch the pack icon when icon_url is provided (new profile).
    // When installing to an existing profile (e.g. server projects),
    // icon_url is None and we preserve the profile's existing icon.
    let icon = if has_icon_url {
        if let Some(icon_url) = project.icon_url {
            let state = State::get().await?;
            reporter
                .set_context(
                    InstallErrorContext::new("download modpack icon")
                        .urls(vec![icon_url.clone()])
                        .project_id(project_id.clone())
                        .version_id(version_id.clone())
                        .build(),
                )
                .await?;
            let icon_bytes = fetch_with_pool(
                &icon_url,
                None,
                None,
                None,
                &state.fetch_semaphore,
                &state.pool,
            )
            .await?;

            let filename = icon_url.rsplit('/').next();

            if let Some(filename) = filename {
                Some(
                    write_cached_icon(
                        filename,
                        &state.directories.caches_dir(),
                        icon_bytes,
                        &state.io_semaphore,
                    )
                    .await?,
                )
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // Set the icon immediately so the UI shows it during download.
    if let Some(ref icon_path) = icon {
        let _ = crate::api::instance::edit_icon(
            &instance_id,
            Some(icon_path.as_path()),
        )
        .await;
    }

    Ok(CreatePack {
        file: CreatePackFile::Path(pack_path),
        description: CreatePackDescription {
            icon,
            override_title: Some(title),
            project_id: Some(project_id),
            version_id: Some(version_id),
            instance_id,
            source_filename: None,
        },
    })
}

fn modpack_cache_path(
    root: &Path,
    project_id: &str,
    version_id: &str,
    filename: &str,
) -> crate::Result<PathBuf> {
    let mut path = root.to_path_buf();
    for segment in [project_id, version_id] {
        if segment.contains('/') {
            return Err(crate::ErrorKind::InputError(
                "Invalid modpack cache path component".to_string(),
            )
            .into());
        }
        path = crate::util::io::safe_relative_join(&path, segment)?;
    }
    let filename = crate::util::io::sanitize_download_file_name(filename)?;
    crate::util::io::safe_relative_join(&path, &filename)
}

#[tracing::instrument]
pub async fn generate_pack_from_file(
    path: PathBuf,
    instance_id: String,
) -> crate::Result<CreatePack> {
    let source_filename =
        path.file_name().map(|x| x.to_string_lossy().to_string());

    Ok(CreatePack {
        file: CreatePackFile::Path(path),
        description: CreatePackDescription {
            icon: None,
            override_title: None,
            project_id: None,
            version_id: None,
            instance_id,
            source_filename,
        },
    })
}

/// Sets generated instance attributes to the pack ones.
/// This includes the pack name, icon, game version, loader version, and loader
pub async fn set_instance_information(
    instance_id: String,
    description: &CreatePackDescription,
    backup_name: &str,
    pack_version_id: Option<&str>,
    dependencies: &HashMap<PackDependency, String>,
    _ignore_lock: bool,
) -> crate::Result<()> {
    set_instance_information_with_local_loader_versions(
        instance_id,
        description,
        backup_name,
        pack_version_id,
        dependencies,
        &HashMap::new(),
    )
    .await
}

pub(crate) async fn set_instance_information_with_local_loader_versions(
    instance_id: String,
    description: &CreatePackDescription,
    backup_name: &str,
    pack_version_id: Option<&str>,
    dependencies: &HashMap<PackDependency, String>,
    local_loader_versions: &HashMap<PackDependency, String>,
) -> crate::Result<()> {
    let mut game_version: Option<&String> = None;
    for (key, value) in dependencies {
        if *key == PackDependency::Minecraft {
            game_version = Some(value);
        }
    }

    let Some(game_version) = game_version else {
        return Err(crate::ErrorKind::InputError(
            "Pack did not specify Minecraft version".to_string(),
        )
        .into());
    };

    let primary_dependencies = [
        (PackDependency::Forge, ModLoader::Forge),
        (PackDependency::NeoForge, ModLoader::NeoForge),
        (PackDependency::FabricLoader, ModLoader::Fabric),
        (PackDependency::QuiltLoader, ModLoader::Quilt),
        (PackDependency::Cleanroom, ModLoader::Cleanroom),
        (PackDependency::LegacyFabric, ModLoader::LegacyFabric),
    ]
    .into_iter()
    .filter(|(dependency, _)| dependencies.contains_key(dependency))
    .collect::<Vec<_>>();
    if primary_dependencies.len() > 1 {
        return Err(crate::ErrorKind::InputError(format!(
            "Pack declares incompatible primary loaders: {}",
            primary_dependencies
                .iter()
                .map(|(_, loader)| loader.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .into());
    }

    let primary_loader = primary_dependencies
        .first()
        .map(|(_, loader)| *loader)
        .unwrap_or(ModLoader::Vanilla);
    let mut components = vec![LoaderComponent::new_primary(
        instance_id.clone(),
        primary_loader,
        None,
    )];
    if let Some((dependency, loader)) = primary_dependencies.first() {
        let version = resolve_pack_loader_version(
            dependencies,
            *dependency,
            *loader,
            game_version,
            local_loader_versions,
        )
        .await?;
        components[0].version = Some(version);
        components[0].provider_metadata = Some(serde_json::json!({
            "source": "pack"
        }));
    }
    for (dependency, loader, kind) in [
        (
            PackDependency::LiteLoader,
            ModLoader::LiteLoader,
            LoaderComponentKind::LiteLoader,
        ),
        (
            PackDependency::OptiFine,
            ModLoader::OptiFine,
            LoaderComponentKind::OptiFine,
        ),
    ] {
        if dependencies.contains_key(&dependency) {
            let version = resolve_pack_loader_version(
                dependencies,
                dependency,
                loader,
                game_version,
                local_loader_versions,
            )
            .await?;
            components.push(LoaderComponent {
                instance_id: instance_id.clone(),
                kind,
                version: Some(version),
                role: LoaderComponentRole::Adjunct,
                provider_metadata: Some(serde_json::json!({
                    "source": "pack"
                })),
            });
        }
    }
    if let Some(version) = dependencies
        .get(&PackDependency::OptiFabric)
        .filter(|version| !version.trim().is_empty())
    {
        components.push(LoaderComponent {
            instance_id: instance_id.clone(),
            kind: LoaderComponentKind::OptiFabric,
            version: Some(version.clone()),
            role: LoaderComponentRole::Adjunct,
            provider_metadata: Some(serde_json::json!({
                "projectId": crate::install::runner::OPTIFABRIC_CURSEFORGE_PROJECT_ID,
                "provider": "curseforge",
                "source": "pack"
            })),
        });
    }
    crate::install::runner::validate_loader_components(&components)?;
    let (mod_loader, loader_version) =
        crate::state::project_loader_components(&components)?;

    let link = match (&description.project_id, &description.version_id) {
        (Some(project_id), Some(version_id)) => {
            Some(InstanceLink::ModrinthModpack {
                project_id: project_id.clone(),
                version_id: version_id.clone(),
            })
        }
        _ if description.source_filename.is_some() => {
            Some(InstanceLink::ImportedModpack {
                project_id: None,
                version_id: None,
                name: Some(backup_name.to_string()),
                version_number: pack_version_id.map(ToString::to_string),
                filename: description.source_filename.clone(),
            })
        }
        _ => None,
    };
    let source_kind = match &link {
        Some(InstanceLink::ModrinthModpack { .. }) => {
            Some(ContentSourceKind::ModrinthModpack)
        }
        Some(InstanceLink::ImportedModpack { .. }) => {
            Some(ContentSourceKind::ImportedModpack)
        }
        _ => None,
    };
    crate::api::instance::edit(
        &instance_id,
        EditInstance {
            install_stage: Some(InstanceInstallStage::PackInstalling),
            name: Some(
                description
                    .override_title
                    .clone()
                    .unwrap_or_else(|| backup_name.to_string()),
            ),
            icon_path: description
                .icon
                .as_ref()
                .map(|icon| Some(icon.to_string_lossy().to_string())),
            link,
            content_set_patch: Some(AppliedContentSetPatch {
                source_kind,
                game_version: Some(game_version.clone()),
                protocol_version: Some(None),
                loader: Some(mod_loader),
                loader_version: Some(loader_version),
            }),
            ..EditInstance::default()
        },
    )
    .await?;
    let state = State::get().await?;
    crate::state::instances::commands::replace_instance_loader_components(
        &instance_id,
        &components,
        &state.pool,
    )
    .await?;
    Ok(())
}

async fn resolve_pack_loader_version(
    dependencies: &HashMap<PackDependency, String>,
    dependency: PackDependency,
    loader: ModLoader,
    game_version: &str,
    local_loader_versions: &HashMap<PackDependency, String>,
) -> crate::Result<String> {
    if let Some(version) = local_loader_versions.get(&dependency) {
        return Ok(version.clone());
    }
    let requested = dependencies
        .get(&dependency)
        .filter(|version| !version.trim().is_empty())
        .ok_or_else(|| {
            crate::ErrorKind::InputError(format!(
                "Pack is missing the {} version",
                loader.as_str()
            ))
        })?;
    crate::launcher::get_loader_version_from_profile(
        game_version,
        loader,
        Some(requested),
    )
    .await?
    .map(|version| version.id)
    .ok_or_else(|| {
        crate::ErrorKind::InputError(format!(
            "Loader version {requested} is not available for {} {game_version}",
            loader.as_str()
        ))
        .into()
    })
}

#[cfg(test)]
mod cache_path_tests {
    use super::modpack_cache_path;
    use std::path::Path;

    #[test]
    fn cache_components_cannot_escape_their_directories() {
        let root = Path::new("modpacks");
        assert_eq!(
            modpack_cache_path(root, "project", "version", "pack.mrpack")
                .unwrap(),
            root.join("project/version/pack.mrpack")
        );
        for malicious in [
            "../outside",
            "nested/name",
            r"..\outside",
            "C:/outside",
            r"\\server\share",
            ".",
            "",
            "NUL",
        ] {
            for position in 0..3 {
                let mut components = ["project", "version", "pack.mrpack"];
                components[position] = malicious;
                assert!(
                    modpack_cache_path(
                        root,
                        components[0],
                        components[1],
                        components[2]
                    )
                    .is_err(),
                    "{position}: {malicious:?}"
                );
            }
        }
        for invalid_id in ["C:outside", "pack.mrpack:stream", ".. "] {
            assert!(
                modpack_cache_path(root, invalid_id, "version", "pack.mrpack")
                    .is_err()
            );
            assert!(
                modpack_cache_path(root, "project", invalid_id, "pack.mrpack")
                    .is_err()
            );
        }
    }

    #[test]
    fn remote_modpack_filename_replaces_invalid_characters() {
        let root = Path::new("modpacks");
        let path = modpack_cache_path(
            root,
            "oMMOxJjh",
            "RRYxMDqj",
            "Yet Another Bingo: Ultimate-214.0.mrpack",
        )
        .unwrap();
        assert_eq!(
            path,
            root.join(
                "oMMOxJjh/RRYxMDqj/Yet Another Bingo_ Ultimate-214.0.mrpack"
            )
        );
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join(path.file_name().unwrap());
        std::fs::write(&target, b"pack").unwrap();
        assert_eq!(std::fs::read(target).unwrap(), b"pack");
    }
}
