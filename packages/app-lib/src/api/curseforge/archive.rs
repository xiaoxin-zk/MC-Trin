//! Modpack archive helpers shared by installation and updates.

use crate::api::curseforge::dto::{
    CurseForgeInstallResult, CurseForgeModpackManifest, CurseForgeModpackTarget,
};
use crate::api::curseforge::modpack::CurseForgePackExpectedOverride;
use crate::api::pack::detect::CURSEFORGE_MANIFEST;
use crate::state::ModLoader;
use crate::util::download::constants::OVERRIDE_EXTRACTION_CONCURRENCY;
use crate::{ErrorKind, State};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

pub(crate) struct ModpackOverrideTask {
    pub(crate) index: usize,
    pub(crate) target: PathBuf,
    pub(crate) size: u64,
}

pub(crate) fn collect_modpack_override_tasks<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    manifest: &CurseForgeModpackManifest,
    instance_path: &Path,
    cancellation: Option<&CancellationToken>,
) -> crate::Result<Vec<ModpackOverrideTask>> {
    collect_modpack_override_tasks_at(
        archive,
        manifest,
        instance_path,
        "",
        cancellation,
    )
}

/// Same task collection, but addressing a pack wrapped in one top-level folder.
///
/// The manifest's `overrides` field is relative to the pack root, not to the
/// archive root, so a wrapped pack stores its files under `{base}{overrides}/`.
/// Root packs pass an empty `base` and keep the exact previous prefix.
pub(crate) fn collect_modpack_override_tasks_at<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    manifest: &CurseForgeModpackManifest,
    instance_path: &Path,
    base: &str,
    cancellation: Option<&CancellationToken>,
) -> crate::Result<Vec<ModpackOverrideTask>> {
    let prefix = format!("{base}{}/", manifest.overrides.trim_matches('/'));
    let mut tasks_by_target = HashMap::<PathBuf, ModpackOverrideTask>::new();
    for index in 0..archive.len() {
        crate::api::pack::archive_util::check_cancellation(cancellation)?;
        let entry = archive.by_index(index).map_err(modpack_zip_error)?;
        // Normalize before prefix matching: a Windows-produced archive stores
        // `pack\overrides\config\a.toml`, and comparing that raw against the
        // slash prefix would silently drop every override.
        let entry_name =
            crate::pack::detect::decode_zip_entry_name(entry.name_raw())
                .replace('\\', "/");
        if entry.is_dir() || !entry_name.starts_with(&prefix) {
            continue;
        }
        let relative = &entry_name[prefix.len()..];
        let safe_path = safe_archive_relative_path(relative)?;
        let target = instance_path.join(safe_path);
        // Preserve archive order semantics for duplicate targets: the last
        // entry wins, while unique targets can be extracted independently.
        tasks_by_target.insert(
            target.clone(),
            ModpackOverrideTask {
                index,
                target,
                size: entry.size(),
            },
        );
    }
    let total_size = tasks_by_target
        .values()
        .fold(0_u64, |total, task| total.saturating_add(task.size));
    let limit = crate::api::pack::archive_util::EXTRACTION_SIZE_LIMIT;
    if total_size > limit {
        return Err(ErrorKind::InputError(format!(
            "CurseForge modpack overrides exceed the extraction limit: {total_size} unpacked bytes required, limit is {limit} bytes"
        ))
        .into());
    }
    Ok(tasks_by_target.into_values().collect())
}

pub(crate) fn materialize_modpack_overrides(
    archive_path: &Path,
    instance_path: &Path,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> crate::Result<(
    u32,
    crate::api::pack::archive_util::StagedArchiveReplacements,
)> {
    materialize_modpack_overrides_at(
        archive_path,
        instance_path,
        "",
        cancellation,
    )
}

/// Same override materialisation, addressing a pack wrapped in one folder.
///
/// `base` is the detected pack prefix. Both the manifest lookup and the
/// `overrides` prefix are resolved against it, because a wrapped pack keeps its
/// manifest and override tree below that folder. Root packs pass `""` and keep
/// their previous behaviour exactly.
pub(crate) fn materialize_modpack_overrides_at(
    archive_path: &Path,
    instance_path: &Path,
    base: &str,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> crate::Result<(
    u32,
    crate::api::pack::archive_util::StagedArchiveReplacements,
)> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(modpack_zip_error)?;
    let manifest = read_modpack_manifest_at(&mut archive, base, cancellation)?;
    let tasks = collect_modpack_override_tasks_at(
        &mut archive,
        &manifest,
        instance_path,
        base,
        cancellation,
    )?;
    drop(archive);
    if tasks.is_empty() {
        return Ok((
            0,
            crate::api::pack::archive_util::materialize_staged_archive_entries(
                &[],
                cancellation,
            )?,
        ));
    }
    let targets = tasks
        .iter()
        .map(|task| task.target.clone())
        .collect::<Vec<_>>();
    let worker_count = OVERRIDE_EXTRACTION_CONCURRENCY.min(tasks.len());
    let queue = Arc::new(Mutex::new(VecDeque::from(tasks)));
    let extraction_result = std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            workers.push(scope.spawn(move || -> crate::Result<u32> {
                let file = std::fs::File::open(archive_path).map_err(|error| {
                    crate::util::io::IOError::with_path(error, archive_path)
                })?;
                let mut archive =
                    zip::ZipArchive::new(file).map_err(modpack_zip_error)?;
                let mut files_written = 0_u32;
                loop {
                    crate::api::pack::archive_util::check_cancellation(
                        cancellation,
                    )?;
                    let Some(task) = queue.lock().unwrap().pop_front() else {
                        break;
                    };
                    let mut entry = archive
                        .by_index(task.index)
                        .map_err(modpack_zip_error)?;
                    let written = crate::api::pack::archive_util::write_archive_entry_to_staging(
                        &mut entry,
                        &task.target,
                        cancellation,
                    )?;
                    if written != entry.size() {
                        return Err(ErrorKind::InputError(
                            "CurseForge modpack override was truncated during extraction"
                                .to_string(),
                        )
                        .into());
                    }
                    files_written = files_written.checked_add(1).ok_or_else(|| {
                        ErrorKind::InputError(
                            "CurseForge modpack contains too many override files"
                                .to_string(),
                        )
                    })?;
                }
                Ok(files_written)
            }));
        }
        let mut files_written = 0_u32;
        for worker in workers {
            let count = worker.join().map_err(|_| {
                ErrorKind::OtherError(
                    "CurseForge override extraction worker panicked"
                        .to_string(),
                )
            })??;
            files_written =
                files_written.checked_add(count).ok_or_else(|| {
                    ErrorKind::InputError(
                        "CurseForge modpack contains too many override files"
                            .to_string(),
                    )
                })?;
        }
        Ok(files_written)
    });
    match extraction_result {
        Ok(files_written) => {
            let replacements = crate::api::pack::archive_util::materialize_staged_archive_entries(
                &targets,
                cancellation,
            )?;
            Ok((files_written, replacements))
        }
        Err(error) => {
            if let Err(cleanup_error) =
                crate::api::pack::archive_util::discard_staged_archive_entries(
                    &targets,
                )
            {
                return Err(ErrorKind::OtherError(format!(
                    "{error}; failed to clean staged CurseForge overrides: {cleanup_error}"
                ))
                .into());
            }
            Err(error)
        }
    }
}

#[cfg(test)]
pub(crate) fn extract_modpack_overrides(
    archive_path: &Path,
    instance_path: &Path,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> crate::Result<u32> {
    let (files_written, replacements) = materialize_modpack_overrides(
        archive_path,
        instance_path,
        cancellation,
    )?;
    replacements.finalize()?;
    Ok(files_written)
}

pub(crate) async fn settle_materialized_modpack_overrides(
    instance_id: String,
    cancellation: CancellationToken,
    replacements: crate::api::pack::archive_util::StagedArchiveReplacements,
    commit: bool,
) -> crate::Result<()> {
    crate::api::pack::archive_util::run_blocking_instance_write(
        instance_id,
        cancellation,
        move |_| {
            if commit {
                replacements.finalize()
            } else {
                replacements.rollback()
            }
        },
    )
    .await
}

pub(crate) fn read_modpack_override_content<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    manifest: &CurseForgeModpackManifest,
) -> crate::Result<Vec<CurseForgePackExpectedOverride>> {
    let prefix = format!("{}/", manifest.overrides.trim_matches('/'));
    let mut overrides = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(modpack_zip_error)?;
        let entry_name =
            crate::pack::detect::decode_zip_entry_name(entry.name_raw());
        if entry.is_dir() || !entry_name.starts_with(&prefix) {
            continue;
        }
        let Some(override_content) =
            curseforge_override_content(&entry_name[prefix.len()..])?
        else {
            continue;
        };
        overrides.push(override_content);
    }
    overrides.sort_by(|left, right| {
        left.expected_relative_path
            .cmp(&right.expected_relative_path)
    });
    overrides.dedup_by(|left, right| {
        left.expected_relative_path == right.expected_relative_path
    });
    Ok(overrides)
}

pub(crate) fn curseforge_override_content(
    relative_path: &str,
) -> crate::Result<Option<CurseForgePackExpectedOverride>> {
    let relative_path = safe_archive_relative_path(relative_path)?;
    let Some(project_type) = crate::state::instances::adapters::filesystem::project_type_from_relative_path(
        &relative_path,
    ) else {
        return Ok(None);
    };
    if !crate::state::instances::adapters::filesystem::is_scannable_project_path(
        project_type,
        &relative_path,
    ) {
        return Ok(None);
    }
    Ok(Some(CurseForgePackExpectedOverride {
        project_type,
        expected_relative_path: relative_path,
    }))
}

pub(crate) fn safe_archive_relative_path(value: &str) -> crate::Result<String> {
    let normalized = value.replace('\\', "/");
    let path = Path::new(&normalized);
    if normalized.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ErrorKind::InputError(
            "CurseForge modpack contains an invalid override path".to_string(),
        )
        .into());
    }
    Ok(normalized)
}

/// Largest `manifest.json` this module will parse.
///
/// Real CurseForge manifests are a few KiB; even very large modpacks stay far
/// below a megabyte. The bound stops a member that declares a huge uncompressed
/// size from being inflated into memory during pack identification.
pub(crate) const MAX_MODPACK_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) fn read_modpack_manifest<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> crate::Result<CurseForgeModpackManifest> {
    read_modpack_manifest_cancellable(archive, None)
}

/// Reads `manifest.json` with a size bound and an optional cancellation check.
///
/// The read is capped *while* it happens (`take(limit + 1)`), not afterwards, so
/// a hostile member cannot be materialised in memory first. Cancellation is
/// checked before the read and again after it, because the read itself is a
/// single uninterruptible system call from this worker's point of view.
pub(crate) fn read_modpack_manifest_cancellable<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    cancellation: Option<&CancellationToken>,
) -> crate::Result<CurseForgeModpackManifest> {
    read_modpack_manifest_at(archive, "", cancellation)
}

/// Reads `{base}manifest.json`, preferring the archive root.
///
/// A CurseForge pack may be wrapped in one top-level folder, in which case the
/// manifest is not at the root. Root wins whenever it exists so the unwrapped
/// case keeps its exact previous behaviour; the wrapped location is only tried
/// when the root entry is genuinely absent. Every failure that is not "no such
/// entry" (size limit, cancellation, parse error) stays terminal rather than
/// silently falling through to a second read.
pub(crate) fn read_modpack_manifest_at<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    base: &str,
    cancellation: Option<&CancellationToken>,
) -> crate::Result<CurseForgeModpackManifest> {
    crate::api::pack::archive_util::check_cancellation(cancellation)?;
    // Cap before walking the directory: the lookup below is linear in the entry
    // count, so a pathological archive must be refused here rather than relying
    // on each caller to have checked.
    crate::api::pack::detect::check_entry_limit(archive.len())?;
    // Root first: an unwrapped pack must keep its exact previous behaviour, and
    // a stray `{base}manifest.json` must never shadow a real root manifest.
    //
    // The lookup goes through the shared cancellable index scan rather than
    // `archive.by_name`, so a 200k-entry archive polls the token per entry
    // instead of running one uninterruptible linear scan inside the deadline.
    let mut candidates = vec![CURSEFORGE_MANIFEST.to_string()];
    if !base.is_empty() {
        candidates.push(format!("{base}{CURSEFORGE_MANIFEST}"));
    }
    let mut selected = None;
    for candidate in &candidates {
        if crate::api::pack::detect::find_entry_index_cancellable(
            archive,
            candidate,
            cancellation,
        )?
        .is_some()
        {
            selected = Some(candidate.clone());
            break;
        }
    }
    let Some(entry_name) = selected else {
        return Err(ErrorKind::InputError(
            "CurseForge modpack is missing manifest.json".to_string(),
        )
        .into());
    };
    let index = crate::api::pack::detect::find_entry_index_cancellable(
        archive,
        &entry_name,
        cancellation,
    )?
    .ok_or_else(|| {
        ErrorKind::InputError(
            "CurseForge modpack is missing manifest.json".to_string(),
        )
    })?;
    crate::api::pack::archive_util::check_cancellation(cancellation)?;
    let mut entry = archive.by_index(index).map_err(|_| {
        ErrorKind::InputError(
            "CurseForge modpack is missing manifest.json".to_string(),
        )
    })?;
    let mut contents = Vec::new();
    entry
        .by_ref()
        .take(MAX_MODPACK_MANIFEST_BYTES + 1)
        .read_to_end(&mut contents)?;
    if contents.len() as u64 > MAX_MODPACK_MANIFEST_BYTES {
        return Err(ErrorKind::InputError(format!(
            "CurseForge modpack manifest.json is larger than the {MAX_MODPACK_MANIFEST_BYTES}-byte limit"
        ))
        .into());
    }
    crate::api::pack::archive_util::check_cancellation(cancellation)?;
    // Manifests may carry a UTF-8 BOM from Windows tooling.
    let contents = contents
        .strip_prefix(&[0xEF, 0xBB, 0xBF])
        .unwrap_or(&contents);
    Ok(serde_json::from_slice::<CurseForgeModpackManifest>(
        contents,
    )?)
}

pub(crate) fn game_options_pack_source<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    manifest: &CurseForgeModpackManifest,
) -> crate::Result<Option<crate::api::instance::GameOptionsPackSource>> {
    game_options_pack_source_cancellable(archive, manifest, None)
}

pub(crate) fn game_options_pack_source_cancellable<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    manifest: &CurseForgeModpackManifest,
    cancellation: Option<&CancellationToken>,
) -> crate::Result<Option<crate::api::instance::GameOptionsPackSource>> {
    game_options_pack_source_at(archive, manifest, "", cancellation)
}

/// Base-aware variant: a wrapped pack keeps its override tree below `base`.
///
/// Entry names are normalized to `/` before prefix matching, because a
/// Windows-produced archive stores `pack\overrides\options.txt` and a raw
/// comparison against the slash prefix would miss it entirely.
pub(crate) fn game_options_pack_source_at<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    manifest: &CurseForgeModpackManifest,
    base: &str,
    cancellation: Option<&CancellationToken>,
) -> crate::Result<Option<crate::api::instance::GameOptionsPackSource>> {
    let prefix = format!("{base}{}/", manifest.overrides.trim_matches('/'));
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        crate::api::pack::archive_util::check_cancellation(cancellation)?;
        let entry = archive.by_index(index).map_err(modpack_zip_error)?;
        if entry.is_dir() {
            continue;
        }
        let name = crate::pack::detect::decode_zip_entry_name(entry.name_raw())
            .replace('\\', "/");
        let Some(relative) = name.strip_prefix(&prefix) else {
            continue;
        };
        let relative = safe_archive_relative_path(relative)?;
        if matches!(
            relative.as_str(),
            "options.txt" | "config/yosbr/options.txt"
        ) {
            entries.push((false, relative));
        }
    }
    Ok(crate::api::instance::detect_game_options_pack_source(
        entries
            .iter()
            .map(|(is_client, path)| (*is_client, path.as_str())),
    ))
}

pub(crate) fn modpack_servers<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    manifest: &CurseForgeModpackManifest,
) -> crate::Result<Vec<quartz_nbt::NbtCompound>> {
    modpack_servers_cancellable(archive, manifest, None)
}

pub(crate) fn modpack_servers_cancellable<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    manifest: &CurseForgeModpackManifest,
    cancellation: Option<&CancellationToken>,
) -> crate::Result<Vec<quartz_nbt::NbtCompound>> {
    modpack_servers_at(archive, manifest, "", cancellation)
}

/// Base-aware variant: a wrapped pack keeps `servers.dat` below `base`.
pub(crate) fn modpack_servers_at<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    manifest: &CurseForgeModpackManifest,
    base: &str,
    cancellation: Option<&CancellationToken>,
) -> crate::Result<Vec<quartz_nbt::NbtCompound>> {
    let prefix = format!("{base}{}/", manifest.overrides.trim_matches('/'));
    let mut selected = None;
    for index in 0..archive.len() {
        crate::api::pack::archive_util::check_cancellation(cancellation)?;
        let entry = archive.by_index(index).map_err(modpack_zip_error)?;
        if entry.is_dir() {
            continue;
        }
        let name = crate::pack::detect::decode_zip_entry_name(entry.name_raw())
            .replace('\\', "/");
        if let Some(relative) = name.strip_prefix(&prefix)
            && safe_archive_relative_path(relative)? == "servers.dat"
        {
            selected = Some(index);
        }
    }
    match selected {
        Some(index) => crate::api::instance::synced_servers::read_pack_servers(
            archive.by_index(index).map_err(modpack_zip_error)?,
        ),
        None => Ok(Vec::new()),
    }
}

pub(crate) fn modpack_target(
    manifest: &CurseForgeModpackManifest,
) -> crate::Result<CurseForgeModpackTarget> {
    let Some(manifest_loader) = manifest
        .minecraft
        .mod_loaders
        .iter()
        .find(|loader| loader.primary)
        .or_else(|| manifest.minecraft.mod_loaders.first())
    else {
        return Ok(CurseForgeModpackTarget {
            game_version: manifest.minecraft.version.clone(),
            loader: ModLoader::Vanilla,
            loader_version: None,
        });
    };

    let family = loader_family(&manifest_loader.id);
    let manifest_loader_version = manifest_loader
        .id
        .strip_prefix(family)
        .and_then(|version| version.strip_prefix('-'))
        .filter(|version| !version.is_empty());
    let disguised_cleanroom = family == "forge"
        && manifest.minecraft.version == "1.12.2"
        && manifest_loader_version
            .is_some_and(|version| version.starts_with("0."));
    let loader = match family {
        "cleanroom" => ModLoader::Cleanroom,
        "forge" if disguised_cleanroom => ModLoader::Cleanroom,
        "forge" => ModLoader::Forge,
        "fabric" => ModLoader::Fabric,
        "quilt" => ModLoader::Quilt,
        "neo" | "neoforge" => ModLoader::NeoForge,
        _ => {
            return Err(ErrorKind::InputError(format!(
                "CurseForge modpack uses unsupported loader {}",
                manifest_loader.id
            ))
            .into());
        }
    };
    let loader_version = manifest_loader_version
        .filter(|version| !disguised_cleanroom || version.starts_with("0."))
        .map(str::to_string);

    Ok(CurseForgeModpackTarget {
        game_version: manifest.minecraft.version.clone(),
        loader,
        loader_version,
    })
}

/// Reads the identity fields from a local CurseForge archive before an
/// instance is created, avoiding the temporary default Vanilla metadata.
///
/// Bounded like every other central-directory read on the local pack path: the
/// worker gets a cooperative token and a deadline, because it opens the archive
/// and reads a manifest — two operations that can stall on a slow or contended
/// file. `ZipArchive::new` itself cannot be interrupted, so cancellation is
/// observed after it returns; the token still guarantees this is not a second
/// detached parser that nobody can signal.
pub async fn get_local_modpack_target(
    archive_path: &Path,
) -> crate::Result<(String, CurseForgeModpackTarget)> {
    get_local_modpack_target_at(archive_path, "").await
}

/// Same probe, but also accepting a one-folder wrapper.
///
/// `base` is the detected pack prefix (`""` for the archive root). Without it a
/// valid wrapped CurseForge pack reports "missing manifest.json", which the
/// caller then downgrades to an unknown/modded file and a default Vanilla
/// instance. Root is still tried first, so unwrapped packs behave exactly as
/// before and the manifest location never changes identity semantics.
pub async fn get_local_modpack_target_at(
    archive_path: &Path,
    base: &str,
) -> crate::Result<(String, CurseForgeModpackTarget)> {
    let path = archive_path.to_path_buf();
    let base = base.to_string();
    let cancellation = CancellationToken::new();
    let mut guard = LocalTargetProbeGuard::new(cancellation.clone());
    let worker = tokio::task::spawn_blocking(move || {
        // Inside the worker so the deadline also covers the metadata stat,
        // which blocks on network-backed paths.
        crate::api::pack::detect::check_archive_size(&path)?;
        let file = std::fs::File::open(path)?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(modpack_zip_error)?;
        if cancellation.is_cancelled() {
            return Err(crate::ErrorKind::OtherError(
                "Install was canceled".to_string(),
            )
            .into());
        }
        // Direct callers of this probe may not have checked the entry count, so
        // the cap has to live here too, before any directory walk.
        crate::api::pack::detect::check_entry_limit(archive.len())?;
        // Pass the token into the reader so the size-bounded manifest read also
        // observes a cancellation that lands while it is in flight.
        let manifest =
            read_modpack_manifest_at(&mut archive, &base, Some(&cancellation))?;
        if cancellation.is_cancelled() {
            return Err(crate::ErrorKind::OtherError(
                "Install was canceled".to_string(),
            )
            .into());
        }
        let name = manifest
            .name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "CurseForge Modpack".to_string());
        Ok((name, modpack_target(&manifest)?))
    });

    match tokio::time::timeout(crate::api::pack::detect::DETECT_TIMEOUT, worker)
        .await
    {
        Ok(result) => {
            guard.disarm();
            result?
        }
        Err(_) => {
            guard.cancel();
            Err(crate::ErrorKind::InputError(format!(
                "Timed out after {} seconds while reading the CurseForge modpack archive",
                crate::api::pack::detect::DETECT_TIMEOUT.as_secs()
            ))
            .into())
        }
    }
}

/// Cancels a CURSEFORGE target probe when its awaiting future is dropped.
struct LocalTargetProbeGuard {
    token: CancellationToken,
    active: bool,
}

impl LocalTargetProbeGuard {
    fn new(token: CancellationToken) -> Self {
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

impl Drop for LocalTargetProbeGuard {
    fn drop(&mut self) {
        if self.active {
            self.token.cancel();
        }
    }
}

pub(crate) fn modpack_zip_error(error: zip::result::ZipError) -> crate::Error {
    ErrorKind::InputError(format!(
        "CurseForge modpack archive is invalid: {error}"
    ))
    .into()
}

pub(crate) fn loader_family(loader_id: &str) -> &str {
    loader_id.split('-').next().unwrap_or(loader_id)
}

pub(crate) fn loader_type(loader: &str) -> Option<u32> {
    match loader.trim().to_ascii_lowercase().as_str() {
        "forge" | "cleanroom" => Some(1),
        "fabric" => Some(4),
        "quilt" => Some(5),
        "neoforge" => Some(6),
        _ => None,
    }
}

pub(crate) fn looks_like_game_version(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && loader_type(value).is_none()
}

pub(crate) fn merge_install_result(
    target: &mut CurseForgeInstallResult,
    mut source: CurseForgeInstallResult,
) {
    target.installed.append(&mut source.installed);
    target.manual_downloads.append(&mut source.manual_downloads);
    target.failed_downloads.append(&mut source.failed_downloads);
    target
        .optional_dependencies
        .append(&mut source.optional_dependencies);
    target
        .incompatible_dependencies
        .append(&mut source.incompatible_dependencies);
}

pub(crate) async fn cache_instance_icon_from_url(
    icon_url: &str,
) -> crate::Result<std::path::PathBuf> {
    let normalized =
        crate::util::download::retired_sources::normalize(icon_url)?;
    let icon_url = normalized.as_ref();
    let state = State::get().await?;
    // CurseForge avatar/CDN assets are frequently broken via local system
    // proxies, so always download icons with a direct client.
    let permit = state.fetch_semaphore.0.acquire().await?;
    let response = crate::util::download::native_request::send_cdn_request(
        crate::util::fetch::DIRECT_REQWEST_CLIENT.get(icon_url),
    )
    .await?;
    drop(permit);
    if !response.status().is_success() {
        return Err(ErrorKind::OtherError(format!(
            "CurseForge icon download failed with HTTP {}",
            response.status().as_u16()
        ))
        .into());
    }
    let icon_bytes = response.bytes().await?;
    let filename = icon_url.rsplit('/').next().unwrap_or("icon.png");
    crate::util::fetch::write_cached_icon(
        filename,
        &state.directories.caches_dir(),
        icon_bytes,
        &state.io_semaphore,
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// Builds a stored-only zip so the fixture exercises the same reader the
    /// installer uses.
    fn build_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
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

    const MANIFEST: &str = r#"{
        "minecraft": { "version": "1.20.1", "modLoaders": [ { "id": "forge-47.2.0", "primary": true } ] },
        "manifestType": "minecraftModpack",
        "name": "Wrapped Pack"
    }"#;

    fn read_manifest(
        entries: &[(&str, &[u8])],
        base: &str,
    ) -> crate::Result<CurseForgeModpackManifest> {
        let bytes = build_archive(entries);
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        read_modpack_manifest_at(&mut archive, base, None)
    }

    #[test]
    fn a_root_manifest_is_read_without_a_base() {
        let manifest =
            read_manifest(&[("manifest.json", MANIFEST.as_bytes())], "")
                .expect("root manifest must parse");
        assert_eq!(manifest.name.as_deref(), Some("Wrapped Pack"));
    }

    #[test]
    fn a_wrapped_manifest_is_found_through_the_base_prefix() {
        // A pack zipped inside one folder used to report "missing manifest.json"
        // and downgrade to an unknown file.
        let manifest = read_manifest(
            &[("pack-1.0/manifest.json", MANIFEST.as_bytes())],
            "pack-1.0/",
        )
        .expect("wrapped manifest must parse");
        assert_eq!(manifest.name.as_deref(), Some("Wrapped Pack"));
    }

    #[test]
    fn the_root_manifest_wins_over_a_wrapped_one() {
        // Identity must not change for an archive that has both: root stays
        // authoritative, matching the previous behaviour exactly.
        let root = MANIFEST.replace("Wrapped Pack", "Root Pack");
        let manifest = read_manifest(
            &[
                ("manifest.json", root.as_bytes()),
                ("pack-1.0/manifest.json", MANIFEST.as_bytes()),
            ],
            "pack-1.0/",
        )
        .unwrap();
        assert_eq!(manifest.name.as_deref(), Some("Root Pack"));
    }

    #[test]
    fn a_missing_manifest_reports_missing_not_a_default_identity() {
        // The error must stay classifiable so the caller can tell "not a
        // CurseForge pack" from a real failure.
        let error = read_manifest(&[("other.txt", b"x")], "").unwrap_err();
        assert!(
            error.to_string().contains("missing manifest.json"),
            "got: {error}"
        );
    }

    #[test]
    fn an_oversized_manifest_is_rejected_rather_than_read() {
        let oversized = vec![b' '; (MAX_MODPACK_MANIFEST_BYTES + 1) as usize];
        let error =
            read_manifest(&[("manifest.json", &oversized)], "").unwrap_err();
        assert!(
            error.to_string().contains("larger than the"),
            "got: {error}"
        );
    }

    #[test]
    fn a_wrapped_pack_collects_overrides_below_the_base_prefix() {
        // The manifest's `overrides` field is relative to the pack root, so a
        // wrapped pack keeps its files under `{base}{overrides}/`. Without the
        // base the extractor would find no tasks and silently install nothing.
        let bytes = build_archive(&[
            ("pack-1.0/manifest.json", MANIFEST.as_bytes()),
            ("pack-1.0/overrides/config/a.toml", b"a"),
            ("other/overrides/config/b.toml", b"b"),
        ]);
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let manifest =
            read_modpack_manifest_at(&mut archive, "pack-1.0/", None).unwrap();

        let tasks = collect_modpack_override_tasks_at(
            &mut archive,
            &manifest,
            std::path::Path::new("instance"),
            "pack-1.0/",
            None,
        )
        .unwrap();

        assert_eq!(tasks.len(), 1, "only the wrapped override tree counts");
        assert!(tasks[0].target.ends_with("config/a.toml"));
    }

    #[test]
    fn a_root_pack_still_collects_overrides_without_a_base() {
        let bytes = build_archive(&[
            ("manifest.json", MANIFEST.as_bytes()),
            ("overrides/config/a.toml", b"a"),
        ]);
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let manifest =
            read_modpack_manifest_at(&mut archive, "", None).unwrap();

        let tasks = collect_modpack_override_tasks_at(
            &mut archive,
            &manifest,
            std::path::Path::new("instance"),
            "",
            None,
        )
        .unwrap();

        assert_eq!(tasks.len(), 1);
        assert!(tasks[0].target.ends_with("config/a.toml"));
    }

    #[test]
    fn a_backslash_wrapped_pack_resolves_manifest_and_overrides() {
        // End-to-end for the Windows spelling: the same pack stored with `\`
        // separators must still resolve its manifest and its override tree.
        let bytes = build_archive(&[
            ("pack-1.0\\manifest.json", MANIFEST.as_bytes()),
            ("pack-1.0\\overrides\\config\\a.toml", b"a"),
        ]);
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();

        let manifest =
            read_modpack_manifest_at(&mut archive, "pack-1.0/", None)
                .expect("backslash-wrapped manifest must resolve");
        let tasks = collect_modpack_override_tasks_at(
            &mut archive,
            &manifest,
            std::path::Path::new("instance"),
            "pack-1.0/",
            None,
        )
        .unwrap();

        assert_eq!(tasks.len(), 1, "backslash override must be collected");
        assert!(tasks[0].target.ends_with("config/a.toml"));
    }

    #[test]
    fn wrapped_game_options_are_found_through_the_base() {
        // `options.txt` under a wrapper folder must be recognised as a pack
        // options source; without base + normalization it would be invisible.
        let bytes = build_archive(&[
            ("pack-1.0/manifest.json", MANIFEST.as_bytes()),
            ("pack-1.0/overrides/options.txt", b"guiScale:2"),
        ]);
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let manifest =
            read_modpack_manifest_at(&mut archive, "pack-1.0/", None).unwrap();

        let source = game_options_pack_source_at(
            &mut archive,
            &manifest,
            "pack-1.0/",
            None,
        )
        .unwrap();

        assert!(source.is_some(), "wrapped options.txt must be detected");
    }
}
