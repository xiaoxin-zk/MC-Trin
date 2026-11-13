//! Local modpack file format detection.
//!
//! Detects the modpack format of a local archive by inspecting its contents
//! rather than its file extension, checking both the archive root and a single
//! wrapping folder, mirroring the detection behavior of PCL.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::archive_util::check_cancellation;

const DETECTION_TIMEOUT_ERROR: &str =
    "Timed out while reading modpack archive for format detection";

struct DetectionCancellationGuard {
    token: CancellationToken,
    active: bool,
}

impl DetectionCancellationGuard {
    fn new(token: CancellationToken) -> Self {
        Self {
            token,
            active: true,
        }
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for DetectionCancellationGuard {
    fn drop(&mut self) {
        if self.active {
            self.token.cancel();
        }
    }
}

/// Builds the error the deadline path returns.
///
/// `pub(crate)` rather than private so the install runner can assert that this
/// exact error is classified as terminal instead of as an unknown format.
pub(crate) fn detection_timeout_error() -> crate::Error {
    crate::ErrorKind::InputError(DETECTION_TIMEOUT_ERROR.to_string()).into()
}

pub(crate) fn is_detection_timeout(error: &crate::Error) -> bool {
    let message = error.to_string();
    message.contains(DETECTION_TIMEOUT_ERROR)
        || message.contains("Timed out after")
}

pub const MRPACK_MANIFEST: &str = "modrinth.index.json";
pub const CURSEFORGE_MANIFEST: &str = "manifest.json";
pub const MCBBS_MANIFEST: &str = "mcbbs.packmeta";
pub const HMCL_MANIFEST: &str = "modpack.json";
pub const MMC_MANIFEST: &str = "mmc-pack.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalPackFormat {
    Mrpack,
    CurseForge,
    Mcbbs,
    Hmcl,
    MmcExport,
    LauncherBundled,
    PlainArchive,
    InstanceFolder,
}

impl LocalPackFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mrpack => "Modrinth",
            Self::CurseForge => "CurseForge",
            Self::Mcbbs => "MCBBS",
            Self::Hmcl => "HMCL",
            Self::MmcExport => "MultiMC",
            Self::LauncherBundled => "launcher bundle",
            Self::PlainArchive => "game folder archive",
            Self::InstanceFolder => "instance folder",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DetectedLocalPack {
    pub format: LocalPackFormat,
    /// Prefix of the folder containing the pack's key files, either empty or
    /// a single path segment ending in `/`.
    pub base_folder: String,
    /// For [`LocalPackFormat::LauncherBundled`], the archive entry of the
    /// nested modpack file.
    pub inner_pack_entry: Option<String>,
    /// For [`LocalPackFormat::PlainArchive`], the version id matched under
    /// `versions/<id>/<id>.json`.
    pub plain_version_id: Option<String>,
}

/// Decodes a zip entry name, tolerating archives produced by Chinese tools
/// that store GB18030-encoded names without the UTF-8 flag.
pub fn decode_zip_entry_name(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(name) => name.to_string(),
        Err(_) => {
            let (decoded, _, _) = encoding_rs::GB18030.decode(raw);
            decoded.into_owned()
        }
    }
}

/// Upper bound for inspecting a local pack's central directory.
///
/// Detection only reads entry names plus at most one small manifest. The
/// deadline is cooperative: expiry cancels the worker's token, but a
/// synchronous ZIP constructor already in progress still runs until it returns.
pub const DETECT_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(60);

/// Upper bound for the archive file itself, checked before the constructor runs.
///
/// `ZipArchive::new` cannot be interrupted, so the only way to keep a
/// pathological file from occupying a blocking thread past every deadline is to
/// refuse to open it at all. 8 GiB matches the extraction limit the rest of
/// local pack handling already enforces, so no legitimate pack is rejected.
pub const MAX_DETECT_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Rejects an archive whose on-disk size already exceeds the detection bound.
///
/// Called before `ZipArchive::new` so the uninterruptible constructor never sees
/// a file large enough to make it unresponsive. This is a size guard, not
/// cancellation: a file that passes it can still be slow, which is why the
/// cooperative token and deadline below remain necessary.
pub fn check_archive_size(path: &Path) -> crate::Result<()> {
    let metadata =
        std::fs::metadata(path).map_err(crate::ErrorKind::StdIOError)?;
    if metadata.len() > MAX_DETECT_ARCHIVE_BYTES {
        return Err(crate::ErrorKind::InputError(format!(
            "Modpack archive is {} bytes, which exceeds the {MAX_DETECT_ARCHIVE_BYTES}-byte limit",
            metadata.len()
        ))
        .into());
    }
    Ok(())
}

/// Runs detection with a fresh cancellation token for non-install callers.
pub async fn detect_local_pack_with_timeout(
    path: &Path,
) -> crate::Result<DetectedLocalPack> {
    detect_local_pack(path, &CancellationToken::new()).await
}

/// Shared entry point for callers that already hold an install token.
pub async fn detect_local_pack_bounded(
    path: &Path,
    cancellation: CancellationToken,
) -> crate::Result<DetectedLocalPack> {
    detect_local_pack(path, &cancellation).await
}

pub async fn detect_local_pack(
    path: &Path,
    cancellation: &CancellationToken,
) -> crate::Result<DetectedLocalPack> {
    let path = path.to_path_buf();
    let worker_cancellation = cancellation.child_token();
    let mut cancellation_guard =
        DetectionCancellationGuard::new(worker_cancellation.clone());
    let worker = tokio::task::spawn_blocking(move || {
        detect_local_pack_sync_cancellable(&path, Some(&worker_cancellation))
    });

    match tokio::time::timeout(DETECT_TIMEOUT, worker).await {
        Ok(result) => {
            cancellation_guard.disarm();
            result?
        }
        Err(_) => {
            cancellation_guard.token.cancel();
            Err(detection_timeout_error())
        }
    }
}

fn open_error(path: &Path, error: impl std::fmt::Display) -> crate::Error {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rar"))
    {
        crate::ErrorKind::InputError(
            "RAR modpack archives are not supported; please repackage the modpack as a zip file".to_string(),
        )
        .into()
    } else {
        crate::ErrorKind::InputError(format!(
            "Failed to open modpack archive: {error}"
        ))
        .into()
    }
}

/// Largest entry count detection will walk.
///
/// A real modpack has thousands of entries at most; the largest shipped packs
/// are under ~40k. A zip declaring hundreds of thousands is either hostile or
/// broken, and walking it is what turns detection into a hang. This is a size
/// bound, not a cancellation mechanism: it makes the pathological case fail
/// fast instead of relying on the caller's deadline.
pub const MAX_DETECT_ENTRIES: usize = 200_000;

/// Rejects an archive whose central directory declares too many entries.
///
/// Shared with the CurseForge helpers: any lookup that walks the directory has
/// to refuse a pathological entry count **before** the walk, not rely on the
/// caller having checked. Doing it here keeps every caller's error wording and
/// classification identical.
pub(crate) fn check_entry_limit(total_entries: usize) -> crate::Result<()> {
    if total_entries > MAX_DETECT_ENTRIES {
        return Err(crate::ErrorKind::InputError(format!(
            "Modpack archive has {total_entries} entries, which exceeds the {MAX_DETECT_ENTRIES}-entry detection limit"
        ))
        .into());
    }
    Ok(())
}

/// Largest manifest detection will read into memory.
///
/// The only manifest detection parses is `manifest.json`, which is a few KiB in
/// every real CurseForge pack. This bound stops a member that declares an
/// enormous uncompressed size from filling memory before the timeout matters.
pub const MAX_DETECT_MANIFEST_BYTES: usize = 4 * 1024 * 1024;

pub fn detect_local_pack_sync(path: &Path) -> crate::Result<DetectedLocalPack> {
    detect_local_pack_sync_cancellable(path, None)
}

/// Detection body shared by the sync and cancellable entry points.
///
/// `cancellation` is `None` for the plain sync callers (which run on their own
/// blocking thread and have no deadline to honour) and `Some` for
/// [`detect_local_pack`], which is wrapped in [`DETECT_TIMEOUT`].
pub fn detect_local_pack_sync_cancellable(
    path: &Path,
    cancellation: Option<&CancellationToken>,
) -> crate::Result<DetectedLocalPack> {
    check_cancellation(cancellation)?;
    // Refuse an oversized file before the uninterruptible constructor runs.
    check_archive_size(path)?;
    let file =
        std::fs::File::open(path).map_err(|error| open_error(path, error))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| open_error(path, error))?;
    // Earliest point after the constructor where cancellation is observable.
    check_cancellation(cancellation)?;

    let total_entries = archive.len();
    debug!(
        "detect_local_pack_sync: path={} total_zip_entries={}",
        path.display(),
        total_entries
    );
    check_entry_limit(total_entries)?;

    let mut names = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        check_cancellation(cancellation)?;
        let entry = archive.by_index_raw(index).map_err(|error| {
            crate::ErrorKind::InputError(format!(
                "Failed to read modpack archive entry: {error}"
            ))
        })?;
        if entry.encrypted() {
            return Err(crate::ErrorKind::InputError(
                "Encrypted modpack archives are not supported".to_string(),
            )
            .into());
        }
        let name = decode_zip_entry_name(entry.name_raw());
        debug!(
            "detect_local_pack_sync: scanning entry[{}] name={}",
            index, name
        );
        names.push(name);
    }

    let bases = candidate_bases(&names);
    debug!(
        "detect_local_pack_sync: path={} candidate_bases={:?}",
        path.display(),
        bases
    );
    // Built once so each format probe is O(1) instead of rescanning every name.
    // Detection used to be O(bases x entries) and is now O(entries + bases).
    //
    // Entry names are normalized to `/` before indexing. Windows-produced
    // archives spell the separator the other way, and every probe below builds
    // its target with `/`, so an unnormalized index would simply never match
    // them. Case and base ordering are untouched by normalization.
    let name_index: HashSet<String> =
        names.iter().map(|name| name.replace('\\', "/")).collect();
    let entry_index = build_entry_index(&names);

    for base in &bases {
        check_cancellation(cancellation)?;
        debug!(
            "detect_local_pack_sync: path={} trying base={:?}",
            path.display(),
            base
        );
        if let Some(detected) = detect_at_base_indexed(
            &mut archive,
            &name_index,
            &entry_index,
            base,
            cancellation,
        )? {
            debug!(
                "detect_local_pack_sync: path={} base={:?} matched format={:?}",
                path.display(),
                base,
                detected.format
            );
            return Ok(detected);
        }
        debug!(
            "detect_local_pack_sync: path={} base={:?} no format matched",
            path.display(),
            base
        );
    }

    debug!(
        "detect_local_pack_sync: path={} trying detect_plain_archive",
        path.display()
    );
    if let Some(detected) = detect_plain_archive(&names) {
        debug!(
            "detect_local_pack_sync: path={} matched PlainArchive version_id={:?}",
            path.display(),
            detected.plain_version_id
        );
        return Ok(detected);
    }

    debug!(
        "detect_local_pack_sync: path={} trying detect_instance_folder",
        path.display()
    );
    if let Some(detected) = detect_instance_folder(&names, cancellation)? {
        debug!(
            "detect_local_pack_sync: path={} matched InstanceFolder",
            path.display()
        );
        return Ok(detected);
    }

    debug!(
        "detect_local_pack_sync: path={} no format matched at all",
        path.display()
    );
    Err(crate::ErrorKind::InputError(
        "Unrecognized modpack format: no known pack manifest was found in the archive"
            .to_string(),
    )
    .into())
}

/// The archive root, followed by each distinct single wrapping folder.
///
/// Insertion order is preserved (root first, then first-seen wrappers) because
/// format priority depends on trying the root before any wrapped folder. The
/// set is only an auxiliary membership index; the returned `Vec` is unchanged.
fn candidate_bases(names: &[String]) -> Vec<String> {
    let mut bases = vec![String::new()];
    let mut seen = std::collections::HashSet::new();
    seen.insert(String::new());
    for name in names {
        // Archives written by Windows tooling may separate path segments with
        // `\`. Treat both spellings alike so a wrapped pack is still found; the
        // emitted base always uses `/`, which is what later lookups build.
        let normalized = name.replace('\\', "/");
        if let Some((first, rest)) = normalized.split_once('/')
            && !rest.is_empty()
            && !rest.contains('/')
        {
            let base = format!("{first}/");
            if seen.insert(base.clone()) {
                bases.push(base);
            }
        }
    }
    bases
}

pub(crate) fn detect_at_base<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    names: &[String],
    base: &str,
) -> crate::Result<Option<DetectedLocalPack>> {
    let index: HashSet<String> =
        names.iter().map(|name| name.replace('\\', "/")).collect();
    let entry_index = build_entry_index(names);
    detect_at_base_indexed(archive, &index, &entry_index, base, None)
}

/// Indexed variant used by detection, which builds the set once and reuses it.
///
/// `names` is a set purely for O(1) manifest probing; membership is the only
/// thing consulted, so no ordering is lost (base ordering is decided by the
/// caller's `Vec`, not here).
fn detect_at_base_indexed<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    names: &HashSet<String>,
    entry_index: &HashMap<String, usize>,
    base: &str,
    cancellation: Option<&CancellationToken>,
) -> crate::Result<Option<DetectedLocalPack>> {
    let has = |file: &str| -> bool {
        let target = format!("{base}{file}");
        let found = names.contains(target.as_str());
        debug!(
            "detect_at_base: base={:?} has({}) target={} result={}",
            base, file, target, found
        );
        found
    };
    let detected = |format: LocalPackFormat| DetectedLocalPack {
        format,
        base_folder: base.to_string(),
        inner_pack_entry: None,
        plain_version_id: None,
    };

    // MCBBS and MultiMC packs may also contain a manifest.json, so both must
    // be checked before the CurseForge manifest.
    if has(MCBBS_MANIFEST) {
        debug!(
            "detect_at_base: matched MCBBS via mcbbs.packmeta at base={:?}",
            base
        );
        return Ok(Some(detected(LocalPackFormat::Mcbbs)));
    }
    if has(MMC_MANIFEST) {
        debug!(
            "detect_at_base: matched MmcExport via mmc-pack.json at base={:?}",
            base
        );
        return Ok(Some(detected(LocalPackFormat::MmcExport)));
    }
    if has(MRPACK_MANIFEST) {
        debug!(
            "detect_at_base: matched Mrpack via modrinth.index.json at base={:?}",
            base
        );
        return Ok(Some(detected(LocalPackFormat::Mrpack)));
    }
    if has(CURSEFORGE_MANIFEST) {
        // A manifest.json with an `addons` array is the MCBBS variant. An
        // unreadable manifest.json (e.g. an unrelated mod config inside a
        // zipped game folder) does not abort detection of other formats.
        match read_entry_json(
            archive,
            &format!("{base}{CURSEFORGE_MANIFEST}"),
            entry_index,
            cancellation,
        ) {
            Ok(manifest) => {
                let has_addons = manifest
                    .get("addons")
                    .is_some_and(|value| !value.is_null());
                // A CurseForge manifest carries `minecraft.version`. Without it
                // the file is just some unrelated `manifest.json` (a mod config
                // inside a zipped game folder), and claiming CurseForge here
                // would send the install down a parser that cannot read it.
                let has_minecraft_version = manifest
                    .get("minecraft")
                    .and_then(|minecraft| minecraft.get("version"))
                    .is_some();
                debug!(
                    "detect_at_base: manifest.json at base={:?} addons={} minecraft_version={}",
                    base, has_addons, has_minecraft_version
                );
                if has_addons {
                    return Ok(Some(detected(LocalPackFormat::Mcbbs)));
                }
                if has_minecraft_version {
                    return Ok(Some(detected(LocalPackFormat::CurseForge)));
                }
                // Neither variant: fall through to the remaining probes.
            }
            Err(error) => {
                // A cancellation must propagate instead of being logged as an
                // unparsable manifest, otherwise the walk continues after the
                // caller has given up and the deadline does nothing.
                if cancellation.is_some_and(CancellationToken::is_cancelled) {
                    return Err(error);
                }
                tracing::warn!(
                    "Ignoring unparsable manifest.json at {base:?} during modpack detection: {error}"
                );
            }
        }
    }
    if has(HMCL_MANIFEST) {
        debug!(
            "detect_at_base: matched Hmcl via modpack.json at base={:?}",
            base
        );
        return Ok(Some(detected(LocalPackFormat::Hmcl)));
    }
    for inner in ["modpack.zip", "modpack.mrpack"] {
        if has(inner) {
            debug!(
                "detect_at_base: matched LauncherBundled via {} at base={:?}",
                inner, base
            );
            return Ok(Some(DetectedLocalPack {
                format: LocalPackFormat::LauncherBundled,
                base_folder: base.to_string(),
                inner_pack_entry: Some(format!("{base}{inner}")),
                plain_version_id: None,
            }));
        }
    }

    debug!("detect_at_base: no format matched at base={:?}", base);
    Ok(None)
}

/// Indexes entry names to their archive position, first occurrence winning.
///
/// [`find_entry_index_cancellable`] returns the *first* entry whose normalized
/// name matches, and detection must keep reading that same entry. A plain
/// `collect::<HashMap<_, _>>()` lets a later duplicate overwrite the earlier
/// one, which would change which bytes are parsed for a zip that repeats a
/// manifest name. Keys are normalized exactly like the linear search compares
/// them, so a backslash spelling still resolves to the forward-slash lookups
/// used by the callers.
fn build_entry_index(names: &[String]) -> HashMap<String, usize> {
    let mut index = HashMap::with_capacity(names.len());
    for (position, name) in names.iter().enumerate() {
        match index.entry(name.replace('\\', "/")) {
            Entry::Occupied(_) => {}
            Entry::Vacant(slot) => {
                slot.insert(position);
            }
        }
    }
    index
}

/// Finds an entry index by its decoded name, so lookups stay consistent with
/// [`decode_zip_entry_name`] even for archives with GB18030-encoded names.
pub(crate) fn find_entry_index<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    entry_name: &str,
) -> crate::Result<Option<usize>> {
    find_entry_index_cancellable(archive, entry_name, None)
}

/// Cancellable lookup used where the caller has a deadline to honour.
///
/// The linear scan here can walk every entry (capped at
/// [`MAX_DETECT_ENTRIES`]), so it polls the token per entry: without that, a
/// manifest read bounded elsewhere could still spend its whole budget here
/// without ever observing a cancellation.
pub(crate) fn find_entry_index_cancellable<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    entry_name: &str,
    cancellation: Option<&CancellationToken>,
) -> crate::Result<Option<usize>> {
    for index in 0..archive.len() {
        check_cancellation(cancellation)?;
        let entry = archive.by_index_raw(index).map_err(|error| {
            crate::ErrorKind::InputError(format!(
                "Failed to read modpack archive entry: {error}"
            ))
        })?;
        if decode_zip_entry_name(entry.name_raw()).replace('\\', "/")
            == entry_name
        {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

fn read_entry_json<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    entry_name: &str,
    entry_index: &HashMap<String, usize>,
    cancellation: Option<&CancellationToken>,
) -> crate::Result<serde_json::Value> {
    check_cancellation(cancellation)?;
    let index = entry_index.get(entry_name).copied().ok_or_else(|| {
        crate::ErrorKind::InputError(format!(
            "Modpack archive is missing {entry_name}"
        ))
    })?;
    let mut entry = archive.by_index(index).map_err(|error| {
        crate::ErrorKind::InputError(format!(
            "Failed to read {entry_name} from modpack archive: {error}"
        ))
    })?;
    let mut contents = Vec::new();
    // Bound the read itself rather than checking afterwards: an entry that
    // declares a huge uncompressed size would otherwise be fully materialised
    // in memory before any size check could run.
    entry
        .by_ref()
        .take(MAX_DETECT_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut contents)?;
    if contents.len() > MAX_DETECT_MANIFEST_BYTES {
        return Err(crate::ErrorKind::InputError(format!(
            "Modpack manifest {entry_name} is larger than the {MAX_DETECT_MANIFEST_BYTES}-byte detection limit"
        ))
        .into());
    }
    // Re-check after the read. The `read_to_end` above is a single
    // uninterruptible system read from this worker's point of view, so a
    // cancellation that lands while it is in flight is only observable here.
    // Without this the caller would have already timed out while detection went
    // on to parse the manifest and possibly report a successful format.
    check_cancellation(cancellation)?;
    // Windows tools often prepend a UTF-8 BOM; serde_json rejects it.
    let contents = contents
        .strip_prefix(&[0xEF, 0xBB, 0xBF])
        .unwrap_or(&contents);
    Ok(serde_json::from_slice(contents)?)
}

/// Looks for a `versions/<id>/<id>.json` structure marking a zipped-up game
/// folder, returning the prefix of the folder containing `versions`.
fn detect_plain_archive(names: &[String]) -> Option<DetectedLocalPack> {
    for name in names {
        // Normalize before splitting: a Windows-produced archive spells the
        // separator as `\`, and `versions/<id>/<id>.json` would never match.
        let normalized = name.replace('\\', "/");
        let segments: Vec<&str> = normalized.split('/').collect();
        if segments.len() < 3 {
            continue;
        }
        let json = segments[segments.len() - 1];
        let version = segments[segments.len() - 2];
        let marker = segments[segments.len() - 3];
        let is_match = marker == "versions"
            && !version.is_empty()
            && json
                .strip_suffix(".json")
                .is_some_and(|stem| stem == version);
        debug!(
            "detect_plain_archive: checking entry={} marker={} version={} json={} is_match={}",
            name, marker, version, json, is_match
        );
        if is_match {
            let base = segments[..segments.len() - 3].join("/");
            let base = if base.is_empty() {
                base
            } else {
                format!("{base}/")
            };
            debug!(
                "detect_plain_archive: matched version={} base={:?}",
                version, base
            );
            return Some(DetectedLocalPack {
                format: LocalPackFormat::PlainArchive,
                base_folder: base,
                inner_pack_entry: None,
                plain_version_id: Some(version.to_string()),
            });
        }
    }
    debug!("detect_plain_archive: no versions pattern matched");
    None
}

/// Looks for a `mods` folder containing `.jar` files, marking a simple instance folder.
///
/// Single pass. Every `mods` segment contributes a candidate base, recorded in
/// first-seen order; an entry that is exactly `mods/<name>.jar` under some base
/// marks that base as having an immediate jar. The old implementation collected
/// the bases into a `HashSet` and then rescanned every entry name once per base,
/// which is quadratic and also had no deterministic order — with 200k entries
/// that alone could outlast the caller's deadline. Ordering now comes from the
/// `Vec`, and the jar lookup is O(1) per entry.
///
/// Cancellation is checked per entry: this runs inside the same blocking walk as
/// the rest of detection, so it must observe the deadline like everything else.
fn detect_instance_folder(
    names: &[String],
    cancellation: Option<&CancellationToken>,
) -> crate::Result<Option<DetectedLocalPack>> {
    // First-seen order of every enclosing `mods` folder, plus a flag for
    // "has at least one `mods/<name>.jar` directly inside it".
    let mut bases: Vec<String> = Vec::new();
    let mut has_immediate_jar: Vec<bool> = Vec::new();
    let mut index_of: HashMap<String, usize> = HashMap::new();

    for name in names {
        check_cancellation(cancellation)?;
        // Same normalization as the other scans, so `mods/<name>.jar` is still
        // recognised when the archive uses `\` separators. The direct-child rule
        // is unchanged: the jar must sit exactly one segment below `mods`.
        let normalized = name.replace('\\', "/");
        let segments: Vec<&str> = normalized.split('/').collect();

        for position in 0..segments.len() {
            if segments[position] != "mods" {
                continue;
            }
            let base = segments[..position].join("/");
            let base = if base.is_empty() {
                base
            } else {
                format!("{base}/")
            };

            let slot = match index_of.get(&base) {
                Some(index) => *index,
                None => {
                    let index = bases.len();
                    bases.push(base.clone());
                    has_immediate_jar.push(false);
                    index_of.insert(base, index);
                    index
                }
            };

            // Exactly `mods/<name>.jar` beneath this base, keeping the original
            // case-insensitive `.jar` suffix rule.
            if segments.len() == position + 2
                && segments[position + 1].to_lowercase().ends_with(".jar")
            {
                has_immediate_jar[slot] = true;
            }
        }
    }

    debug!(
        "detect_instance_folder: found {} unique mods folder(s)",
        bases.len()
    );

    for (index, base) in bases.iter().enumerate() {
        if has_immediate_jar[index] {
            debug!(
                "detect_instance_folder: matched InstanceFolder at base={:?}",
                base
            );
            return Ok(Some(DetectedLocalPack {
                format: LocalPackFormat::InstanceFolder,
                base_folder: base.clone(),
                inner_pack_entry: None,
                plain_version_id: None,
            }));
        }
    }

    debug!("detect_instance_folder: no mods folder with .jar files found");
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal stored-only zip in memory.
    ///
    /// Hand-written so the fixture exercises the same reader detection uses,
    /// without depending on the writer API of the pinned `zip` version.
    fn build_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = 0xffff_ffff_u32;
            for byte in bytes {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    let mask = 0_u32.wrapping_sub(crc & 1);
                    crc = (crc >> 1) ^ (0xedb8_8320 & mask);
                }
            }
            !crc
        }

        let mut out = Vec::new();
        let mut directory = Vec::new();
        let mut count = 0_u16;
        for (name, body) in entries {
            let offset = out.len() as u32;
            let crc = crc32(body);
            let size = body.len() as u32;
            out.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
            out.extend_from_slice(&20_u16.to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(body);

            directory.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
            directory.extend_from_slice(&20_u16.to_le_bytes());
            directory.extend_from_slice(&20_u16.to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&crc.to_le_bytes());
            directory.extend_from_slice(&size.to_le_bytes());
            directory.extend_from_slice(&size.to_le_bytes());
            directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&0_u32.to_le_bytes());
            directory.extend_from_slice(&offset.to_le_bytes());
            directory.extend_from_slice(name.as_bytes());
            count += 1;
        }

        let directory_offset = out.len() as u32;
        let directory_size = directory.len() as u32;
        out.extend_from_slice(&directory);
        out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&directory_size.to_le_bytes());
        out.extend_from_slice(&directory_offset.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out
    }

    fn write_pack(
        entries: &[(&str, &[u8])],
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pack.zip");
        std::fs::write(&path, build_archive(entries)).unwrap();
        (dir, path)
    }

    #[test]
    fn detects_every_supported_format_and_keeps_priority() {
        // One fixture per format, plus the two ordering rules that matter:
        // MCBBS wins over a bare manifest.json, and a manifest.json with an
        // `addons` array is MCBBS rather than CurseForge.
        let cases: Vec<(&str, Vec<(&str, &[u8])>, LocalPackFormat)> = vec![
            (
                "mrpack",
                vec![("modrinth.index.json", b"{}" as &[u8])],
                LocalPackFormat::Mrpack,
            ),
            (
                "curseforge",
                vec![(
                    "manifest.json",
                    br#"{"manifestType":"minecraftModpack","minecraft":{"version":"1.20.1"}}"#
                        as &[u8],
                )],
                LocalPackFormat::CurseForge,
            ),
            (
                "curseforge-with-addons-is-mcbbs",
                vec![("manifest.json", br#"{"addons":[]}"# as &[u8])],
                LocalPackFormat::Mcbbs,
            ),
            (
                "mcbbs-beats-manifest",
                vec![
                    ("mcbbs.packmeta", b"{}" as &[u8]),
                    ("manifest.json", br#"{"addons":null}"# as &[u8]),
                ],
                LocalPackFormat::Mcbbs,
            ),
            (
                "mmc",
                vec![("mmc-pack.json", b"{}" as &[u8])],
                LocalPackFormat::MmcExport,
            ),
            (
                "hmcl",
                vec![("modpack.json", b"{}" as &[u8])],
                LocalPackFormat::Hmcl,
            ),
            (
                "launcher-bundled",
                vec![("modpack.zip", b"PK" as &[u8])],
                LocalPackFormat::LauncherBundled,
            ),
            (
                "plain-archive",
                vec![("versions/1.20.1/1.20.1.json", b"{}" as &[u8])],
                LocalPackFormat::PlainArchive,
            ),
        ];

        for (label, entries, expected) in cases {
            let (_dir, path) = write_pack(&entries);
            let detected = detect_local_pack_sync(&path)
                .unwrap_or_else(|error| panic!("{label}: {error}"));
            assert_eq!(detected.format, expected, "{label}");
        }
    }

    #[test]
    fn a_wrapping_folder_is_detected_and_reported() {
        let (_dir, path) =
            write_pack(&[("pack-1.0/modrinth.index.json", b"{}")]);

        let detected = detect_local_pack_sync(&path).unwrap();

        assert_eq!(detected.format, LocalPackFormat::Mrpack);
        assert_eq!(detected.base_folder, "pack-1.0/");
    }

    #[test]
    fn a_backslash_wrapped_pack_is_detected_end_to_end() {
        // Windows-produced archives spell separators as `\\`. Detection must
        // still find the wrapper base and the manifest, otherwise a valid pack
        // is reported as "Unrecognized modpack format".
        let (_dir, path) =
            write_pack(&[("pack-1.0\\modrinth.index.json", b"{}")]);

        let detected = detect_local_pack_sync(&path).unwrap();

        assert_eq!(detected.format, LocalPackFormat::Mrpack);
        assert_eq!(detected.base_folder, "pack-1.0/");
    }

    #[test]
    fn a_backslash_plain_archive_is_detected_end_to_end() {
        // The PlainArchive probe splits on `/`, so a backslash spelling has to
        // be normalized before the `versions/<id>/<id>.json` check.
        let (_dir, path) =
            write_pack(&[("pack\\versions\\1.20.1\\1.20.1.json", b"{}")]);

        let detected = detect_local_pack_sync(&path).unwrap();

        assert_eq!(detected.format, LocalPackFormat::PlainArchive);
        assert_eq!(detected.plain_version_id.as_deref(), Some("1.20.1"));
    }

    #[test]
    fn an_unknown_archive_reports_the_shared_unrecognized_message() {
        // `install/runner.rs` maps this exact prefix to `unrecognized_format`,
        // so the wording is part of the contract, not incidental.
        let (_dir, path) = write_pack(&[("readme.txt", b"nothing")]);

        let error = detect_local_pack_sync(&path).unwrap_err();

        assert!(
            error.to_string().contains("Unrecognized modpack format"),
            "got: {error}"
        );
    }

    #[test]
    fn a_manifest_with_addons_is_mcbbs() {
        // The MCBBS variant is identified by its `addons` array, regardless of
        // whether it also carries a `minecraft` object.
        let (_dir, path) = write_pack(&[(
            "manifest.json",
            br#"{"addons":[{"id":"game","version":"1.20.1"}]}"#,
        )]);

        let detected = detect_local_pack_sync(&path).unwrap();

        assert_eq!(detected.format, LocalPackFormat::Mcbbs);
    }

    #[test]
    fn a_manifest_with_minecraft_version_and_no_addons_is_curseforge() {
        // A real CurseForge manifest: `minecraft.version` plus `modLoaders`, and
        // no `addons` key at all. This is the shape every CF pack ships.
        let (_dir, path) = write_pack(&[(
            "manifest.json",
            br#"{"manifestType":"minecraftModpack","manifestVersion":1,"name":"Pack","version":"1.0.0","author":"someone","minecraft":{"version":"1.20.1","modLoaders":[{"id":"forge-47.2.0","primary":true}]},"files":[]}"#,
        )]);

        let detected = detect_local_pack_sync(&path).unwrap();

        assert_eq!(detected.format, LocalPackFormat::CurseForge);
    }

    #[test]
    fn a_manifest_without_addons_or_minecraft_version_is_not_curseforge() {
        // An unrelated `manifest.json` (a mod config inside a zipped game
        // folder) must not be claimed as CurseForge: the CF installer would then
        // fail on a file it cannot parse. Detection has to keep probing and end
        // with the shared "unrecognized" answer.
        let (_dir, path) = write_pack(&[(
            "manifest.json",
            br#"{"name":"some-mod","version":"2.1.0"}"#,
        )]);

        let error = detect_local_pack_sync(&path).unwrap_err();

        assert!(
            error.to_string().contains("Unrecognized modpack format"),
            "an unrelated manifest.json must not be detected as CurseForge, got: {error}"
        );
    }

    #[test]
    fn a_manifest_with_minecraft_but_no_version_is_not_curseforge() {
        // Guards the exact key the probe reads: a `minecraft` object alone is
        // not enough, `minecraft.version` is what the CF parser needs.
        let (_dir, path) = write_pack(&[(
            "manifest.json",
            br#"{"minecraft":{"modLoaders":[]}}"#,
        )]);

        let error = detect_local_pack_sync(&path).unwrap_err();

        assert!(
            error.to_string().contains("Unrecognized modpack format"),
            "minecraft without version must not be CurseForge, got: {error}"
        );
    }

    #[test]
    fn a_cancelled_token_stops_the_entry_scan() {
        // The cooperative half of the deadline: an explicit cancellation must
        // reach the walk, otherwise the timeout would abandon a worker that
        // keeps reading.
        let (_dir, path) = write_pack(&[("modrinth.index.json", b"{}")]);
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error =
            detect_local_pack_sync_cancellable(&path, Some(&cancellation))
                .unwrap_err();

        assert!(
            error.to_string().contains("canceled"),
            "expected the cancellation error, got: {error}"
        );
    }

    #[test]
    fn detection_runs_normally_without_cancellation() {
        let (_dir, path) = write_pack(&[("modrinth.index.json", b"{}")]);

        let detected = detect_local_pack_sync_cancellable(&path, None).unwrap();

        assert_eq!(detected.format, LocalPackFormat::Mrpack);
    }

    #[test]
    fn a_manifest_over_the_size_limit_is_rejected_by_the_reader() {
        // Exercised through `read_entry_json` directly: detection deliberately
        // swallows an unreadable manifest.json so an unrelated file cannot
        // abort format matching, which would hide this error at the caller.
        let oversized = vec![b' '; MAX_DETECT_MANIFEST_BYTES + 1];
        let (_dir, path) = write_pack(&[("manifest.json", &oversized)]);
        let mut archive =
            zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();

        let names = vec!["manifest.json".to_string()];
        let entry_index = build_entry_index(&names);

        let error =
            read_entry_json(&mut archive, "manifest.json", &entry_index, None)
                .unwrap_err();

        assert!(
            error.to_string().contains("detection limit"),
            "expected the size-limit error, got: {error}"
        );
    }

    #[test]
    fn entry_limit_rejects_only_counts_above_the_cap() {
        assert!(check_entry_limit(MAX_DETECT_ENTRIES).is_ok());
        let error = check_entry_limit(MAX_DETECT_ENTRIES + 1).unwrap_err();
        assert!(error.to_string().contains("detection limit"));
    }

    #[test]
    fn candidate_base_and_entry_indexes_preserve_order_and_lookup() {
        let names = vec![
            "first/other.txt".to_string(),
            "second/other.txt".to_string(),
            "first/modrinth.index.json".to_string(),
        ];
        let bases = candidate_bases(&names);
        let entry_index = build_entry_index(&names);

        assert_eq!(bases, ["", "first/", "second/"]);
        assert_eq!(entry_index.get("first/modrinth.index.json"), Some(&2));
    }

    #[test]
    fn entry_index_keeps_the_first_duplicate_like_the_linear_search() {
        // `find_entry_index_cancellable` returns the first match, so the index
        // has to point at the same entry. A plain `collect` would keep the last
        // duplicate and silently change which bytes a manifest lookup parses.
        let names = vec![
            "manifest.json".to_string(),
            "readme.txt".to_string(),
            "manifest.json".to_string(),
        ];
        let entry_index = build_entry_index(&names);

        assert_eq!(entry_index.get("manifest.json"), Some(&0));
    }

    #[test]
    fn entry_index_normalizes_backslashes_like_the_linear_search() {
        // Detection looks entries up with forward slashes. Archives written by
        // Windows tools can spell the separator the other way, and the linear
        // search normalizes before comparing, so the index must too.
        let names = vec!["overrides\\config\\thing.toml".to_string()];
        let entry_index = build_entry_index(&names);

        assert_eq!(entry_index.get("overrides/config/thing.toml"), Some(&0));
        assert_eq!(entry_index.get("overrides\\config\\thing.toml"), None);
    }

    #[test]
    fn timeout_errors_are_classified_for_terminal_handling() {
        let error = detection_timeout_error();
        assert!(is_detection_timeout(&error));
        assert!(error.to_string().contains("Timed out"));
    }

    #[test]
    fn cancellation_guard_signals_worker_token_on_drop() {
        let token = CancellationToken::new();
        let worker_token = token.child_token();

        drop(DetectionCancellationGuard::new(worker_token.clone()));

        assert!(worker_token.is_cancelled());
    }

    #[tokio::test]
    async fn timeout_path_returns_detection_timeout_error_and_cancels_worker() {
        let (_dir, path) = write_pack(&[("modrinth.index.json", b"{}")]);
        let token = CancellationToken::new();
        let worker_token = token.child_token();
        let worker_path = path.clone();
        let worker_for_task = worker_token.clone();
        let (release_worker, wait_for_release) = std::sync::mpsc::channel();
        let (worker_started, wait_for_worker) = tokio::sync::oneshot::channel();
        let mut guard = DetectionCancellationGuard::new(worker_token.clone());
        let worker = tokio::task::spawn_blocking(move || {
            let _ = worker_started.send(());
            let _ = wait_for_release.recv();
            detect_local_pack_sync_cancellable(
                &worker_path,
                Some(&worker_for_task),
            )
        });
        wait_for_worker.await.unwrap();

        let result =
            tokio::time::timeout(std::time::Duration::ZERO, worker).await;
        assert!(result.is_err());
        guard.token.cancel();
        release_worker.send(()).unwrap();
        let error = detection_timeout_error();

        assert!(is_detection_timeout(&error));
        assert!(worker_token.is_cancelled());
    }

    #[test]
    fn instance_folder_matches_the_first_mods_base_that_has_a_jar() {
        // Two candidate bases, only the later one has an immediate jar. The
        // result must be that base, and it must be chosen by first-seen order
        // rather than by hash iteration order.
        let names = vec![
            "alpha/mods/".to_string(),
            "alpha/mods/nested/pack.jar".to_string(),
            "beta/mods/".to_string(),
            "beta/mods/real-pack.JAR".to_string(),
        ];

        let detected = detect_instance_folder(&names, None)
            .unwrap()
            .expect("beta has an immediate jar");

        assert_eq!(detected.format, LocalPackFormat::InstanceFolder);
        assert_eq!(detected.base_folder, "beta/");
    }

    #[test]
    fn instance_folder_prefers_the_earliest_base_when_both_have_jars() {
        // Deterministic ordering: with two qualifying bases the first one seen
        // wins, which a HashSet could not guarantee.
        let names = vec![
            "first/mods/a.jar".to_string(),
            "second/mods/b.jar".to_string(),
        ];

        let detected = detect_instance_folder(&names, None).unwrap().unwrap();

        assert_eq!(detected.base_folder, "first/");
    }

    #[test]
    fn instance_folder_without_an_immediate_jar_is_not_a_match() {
        // A `mods` folder that only holds nested content must not be promoted:
        // the old rule required `mods/<name>.jar` directly beneath the base.
        let names = vec![
            "pack/mods/nested/deep.jar".to_string(),
            "pack/config/thing.txt".to_string(),
        ];

        assert!(detect_instance_folder(&names, None).unwrap().is_none());
    }

    #[test]
    fn instance_folder_ignores_non_jar_files_directly_in_mods() {
        let names = vec![
            "pack/mods/readme.txt".to_string(),
            "pack/mods/disabled.jar.txt".to_string(),
        ];

        assert!(detect_instance_folder(&names, None).unwrap().is_none());
    }

    #[test]
    fn instance_folder_without_any_mods_folder_is_not_a_match() {
        let names = vec!["pack/config/a.toml".to_string()];

        assert!(detect_instance_folder(&names, None).unwrap().is_none());
    }

    #[test]
    fn instance_folder_honours_cancellation() {
        // The single pass must observe the deadline like the rest of detection.
        let names = vec!["pack/mods/a.jar".to_string()];
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error =
            detect_instance_folder(&names, Some(&cancellation)).unwrap_err();

        assert!(
            error.to_string().contains("canceled"),
            "expected the cancellation error, got: {error}"
        );
    }

    #[test]
    fn instance_folder_detects_a_root_level_mods_folder() {
        // The empty base means the archive root, and the returned base stays
        // empty rather than becoming "/".
        let names = vec!["mods/a.jar".to_string()];

        let detected = detect_instance_folder(&names, None).unwrap().unwrap();

        assert_eq!(detected.base_folder, "");
    }
}
