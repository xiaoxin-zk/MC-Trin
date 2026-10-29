//! Unified content-type classifier for dropped / imported files.
//!
//! Determines what kind of Minecraft content a file or folder represents,
//! supporting launcher directories, mod JARs, resource packs, world saves,
//! litematic files, shader packs, and more.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::pack::detect::LocalPackFormat;
use crate::api::pack::import::ImportLauncherType;
use crate::mod_metadata::manifest::read_jar_manifest;
use crate::state::{ModrinthProjectId, ModrinthVersionId};
use tokio_util::sync::CancellationToken;

/// Maximum number of items allowed in a ZIP before we classify it as "ZIP
/// with many items" rather than "single file/folder wrapped in ZIP".
const ZIP_TOP_LEVEL_LIMIT: usize = 200;
const MAX_ZIP_EXTRACTION_ENTRIES: usize = 100_000;
const MAX_ZIP_EXTRACTION_BYTES: u64 = 32 * 1024 * 1024 * 1024;

fn is_cancelled(cancellation: Option<&CancellationToken>) -> bool {
    cancellation.is_some_and(CancellationToken::is_cancelled)
}

fn cancelled_result() -> DroppedItemType {
    DroppedItemType::Unknown {
        reason: "Operation cancelled".to_string(),
    }
}

/// Entry-name segments that are never descended into during classification.
/// They are operating-system noise, not Minecraft content.
const NOISE_ENTRY_NAMES: [&str; 4] =
    ["__MACOSX", ".git", "Thumbs.db", ".DS_Store"];

/// Result of classifying a file path dropped / imported by the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DroppedItemType {
    /// A recognised third-party launcher root folder.
    Launcher {
        launcher_type: ImportLauncherType,
        base_path: PathBuf,
        /// For ZIP sources, the virtual folder inside the archive where the
        /// launcher markers matched (e.g. `.minecraft`). The frontend
        /// extracts the archive and scans this subfolder. `None` for real
        /// folders and files.
        inner_base: Option<String>,
    },
    /// HMCL launcher with separate launcher and data directories.
    HmclLauncher {
        launcher_dir: PathBuf,
        data_dir: PathBuf,
    },
    /// A mod JAR file.
    Mod { file_path: PathBuf },
    /// A `.litematic` or `.schematic` file.
    Litematic { file_path: PathBuf },
    /// A resource pack and/or data pack.
    ResourcePack {
        file_path: PathBuf,
        candidates: Vec<String>,
        inner_base: Option<String>,
    },
    /// A shader pack.
    ShaderPack {
        file_path: PathBuf,
        inner_base: Option<String>,
    },
    /// A Minecraft world save folder or archive.
    WorldSave {
        file_path: PathBuf,
        inner_base: Option<String>,
    },
    /// A shortcut / symlink that was resolved to another item type.
    ShortcutResolved {
        original: PathBuf,
        resolved_to: Box<DroppedItemType>,
    },
    /// A modpack archive (.mrpack, CurseForge, MultiMC, etc.).
    Modpack { file_path: PathBuf },
    /// Multiple import candidates found in one source, ordered by priority.
    Multiple {
        file_path: PathBuf,
        candidates: Vec<DroppedCandidate>,
    },
    /// Could not be classified.
    Unknown { reason: String },
}

/// One importable candidate inside a multi-candidate classification result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroppedCandidate {
    pub item_type: String,
    pub file_path: PathBuf,
    pub inner_base: Option<String>,
    pub candidates: Vec<String>,
}

/// Classify a dropped file or folder path into a `DroppedItemType`.
///
/// The classification follows a strict priority order: shortcut resolution,
/// ZIP / EXE / JAR detection, then directory and file fallbacks.
/// Returns `Unknown` instead of panicking on any error.
pub fn classify_dropped_item(path: &Path) -> DroppedItemType {
    classify_dropped_item_inner(path, 0, true, None)
}

/// Classify a dropped item while cooperating with a caller-controlled
/// cancellation token. The synchronous implementation checks the token
/// between filesystem and archive operations so a timed-out blocking worker
/// can finish before its caller reports failure or cleans up its inputs.
pub fn classify_dropped_item_cancellable(
    path: &Path,
    cancellation: &CancellationToken,
) -> DroppedItemType {
    classify_dropped_item_inner(path, 0, true, Some(cancellation))
}

/// Like [`classify_dropped_item`] but never unpacks nested archives: when a
/// nested archive would have to be unpacked to decide the type, the result
/// is `Unknown` with a reason reporting the total nested size, so the caller
/// can confirm the (potentially slow) unpack with the user first.
pub fn classify_dropped_item_without_nested_unpack(
    path: &Path,
) -> DroppedItemType {
    classify_dropped_item_inner(path, 0, false, None)
}

/// Maximum number of shortcut hops followed before giving up. Guards against
/// shortcut / symlink cycles (e.g. `a.lnk` → `b.lnk` → `a.lnk`) that would
/// otherwise recurse until the stack overflows.
const MAX_SHORTCUT_HOPS: u32 = 8;

fn classify_dropped_item_inner(
    path: &Path,
    shortcut_depth: u32,
    allow_nested_unpack: bool,
    cancellation: Option<&CancellationToken>,
) -> DroppedItemType {
    if is_cancelled(cancellation) {
        return cancelled_result();
    }
    if !path.exists() {
        let reason = "Path does not exist".to_string();
        tracing::warn!(
            "Classification failed for '{}': {reason}",
            path.display()
        );
        return DroppedItemType::Unknown { reason };
    }

    if let Some(resolved) =
        crate::util::resolve_shortcut::resolve_shortcut(path, 3)
        && resolved != path
        && shortcut_depth < MAX_SHORTCUT_HOPS
    {
        let inner = classify_dropped_item_inner(
            &resolved,
            shortcut_depth + 1,
            allow_nested_unpack,
            cancellation,
        );
        return DroppedItemType::ShortcutResolved {
            original: path.to_path_buf(),
            resolved_to: Box::new(inner),
        };
    }

    if is_zip_path(path) {
        return classify_zip_path(path, allow_nested_unpack, cancellation);
    }

    if let Some(ext) = path.extension()
        && ext.eq_ignore_ascii_case("exe")
    {
        return classify_launcher_exe(path);
    }

    if let Some(ext) = path.extension()
        && ext.eq_ignore_ascii_case("disabled")
    {
        return classify_disabled(path, allow_nested_unpack, cancellation);
    }

    if let Some(ext) = path.extension()
        && ext.eq_ignore_ascii_case("jar")
    {
        return classify_jar(path);
    }

    // Step 5: Directory.
    if path.is_dir() {
        let result = classify_folder(path, allow_nested_unpack, cancellation);
        tracing::debug!(
            "classify_dropped_item: directory path={} result={:?}",
            path.display(),
            result
        );
        return result;
    }

    let result = classify_file(path);
    tracing::debug!(
        "classify_dropped_item: file path={} result={:?}",
        path.display(),
        result
    );
    result
}

/// Returns true when the path points to a ZIP-family archive (.zip / .mrpack).
fn is_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("zip")
                || ext.eq_ignore_ascii_case("mrpack")
        })
}

/// Classify a `.disabled` file by treating it as the underlying file type
/// (e.g. `mod.jar.disabled` → Mod, `pack.zip.disabled` → ZIP). The original
/// path is kept in the result — no path rewrite happens.
fn classify_disabled(
    path: &Path,
    allow_nested_unpack: bool,
    cancellation: Option<&CancellationToken>,
) -> DroppedItemType {
    let Some(stem) = path.file_stem() else {
        return classify_file(path);
    };
    let Some(stem_str) = stem.to_str() else {
        return classify_file(path);
    };
    let Some(underlying_ext) = stem_str.rsplit('.').next() else {
        return classify_file(path);
    };

    if underlying_ext.eq_ignore_ascii_case("jar") {
        // classify_jar reads the original file content (still valid).
        return classify_jar(path);
    }
    if underlying_ext.eq_ignore_ascii_case("zip")
        || underlying_ext.eq_ignore_ascii_case("mrpack")
    {
        // The file content is still a valid archive, so the ZIP pipeline runs
        // on the original path.
        return classify_zip_path(path, allow_nested_unpack, cancellation);
    }

    // Other .disabled extensions fall through to file classification.
    classify_file(path)
}

// ─── ZIP archive classification ─────────────────────────────────────────────

/// Maximum depth for recursively classifying ZIP contents. Folder nesting
/// inside the archive and nested ZIP files both count towards the limit.
const MAX_ZIP_NESTING_DEPTH: u32 = 5;

/// Entry names of a ZIP archive, enumerated once without extracting anything.
///
/// Folder nesting is handled virtually: each recursion level reuses this set
/// and only narrows the path prefix, so no directory tree is written to disk
/// just to classify it.
struct ZipEntrySet {
    /// Decoded entry names exactly as stored (directories keep the trailing
    /// `/`), used for pack-manifest detection.
    names: Vec<String>,
    /// File paths without a trailing slash.
    files: std::collections::HashSet<String>,
    /// Directory paths without a trailing slash.
    dirs: std::collections::HashSet<String>,
    /// Whether any entry is encrypted (password-protected) and therefore
    /// unreadable during content confirmation.
    has_encrypted: bool,
}

impl ZipEntrySet {
    fn from_archive_with_cancellation<R: std::io::Read + std::io::Seek>(
        archive: &mut zip::ZipArchive<R>,
        cancellation: Option<&CancellationToken>,
    ) -> Result<ZipEntrySet, String> {
        let mut set = ZipEntrySet {
            names: Vec::new(),
            files: std::collections::HashSet::new(),
            dirs: std::collections::HashSet::new(),
            has_encrypted: false,
        };
        for i in 0..archive.len() {
            if is_cancelled(cancellation) {
                return Err("Operation cancelled".to_string());
            }
            let Ok(entry) = archive.by_index_raw(i) else {
                continue;
            };
            if i >= MAX_ZIP_EXTRACTION_ENTRIES {
                return Err(format!(
                    "ZIP contains more than {MAX_ZIP_EXTRACTION_ENTRIES} entries"
                ));
            }
            if entry.encrypted() {
                set.has_encrypted = true;
            }
            // Normalize Windows-authored separators so every lookup and the
            // virtual folder tree agree on one representation.
            let name = crate::api::pack::detect::decode_zip_entry_name(
                entry.name_raw(),
            )
            .replace('\\', "/");
            if name.is_empty() {
                continue;
            }
            set.names.push(name.clone());
            if name.ends_with('/') {
                let normalized = name.trim_end_matches('/').to_string();
                if !normalized.is_empty() {
                    set.dirs.insert(normalized);
                }
            } else {
                set.files.insert(name);
            }
        }
        Ok(set)
    }

    /// Whether a file exists at `{base}{relative}`.
    fn has_file(&self, base: &str, relative: &str) -> bool {
        self.files.contains(&format!("{base}{relative}"))
    }

    /// Whether `versions/<id>/<id>.json` exists under `base` (vanilla
    /// launcher instance marker).
    fn has_version_json(&self, base: &str) -> bool {
        self.files.iter().any(|path| {
            let Some(rest) = path.strip_prefix(base) else {
                return false;
            };
            let parts: Vec<&str> = rest.split('/').collect();
            parts.len() == 3
                && parts[0] == "versions"
                && !parts[1].is_empty()
                && parts[1]
                    == parts[2].strip_suffix(".json").unwrap_or_default()
        })
    }

    /// Whether `axolotl_config.json` exists under `base`.
    fn has_axolotl_config(&self, base: &str) -> bool {
        self.has_file(base, "axolotl_config.json")
    }

    /// Whether a direct child folder under `base` has `axolotl_config.json`
    /// (a container of Axolotl instances).
    fn has_direct_axolotl_config(&self, base: &str) -> bool {
        self.child_folders(base).iter().any(|child| {
            self.has_file(&format!("{base}{child}/"), "axolotl_config.json")
        })
    }

    /// Whether direct child folders under `base` look like version folders
    /// (`<id>/<id>.json`). A base folder named `versions`, at least two
    /// matching children, or a matching child with the same-name `.jar`
    /// makes the classification specific enough to avoid false positives.
    fn has_direct_version_json(&self, base: &str) -> bool {
        let mut matching: Vec<String> = Vec::new();
        for child in self.child_folders(base) {
            let child_base = format!("{base}{child}/");
            if self.has_file(&child_base, &format!("{child}.json")) {
                matching.push(child);
            }
        }
        if matching.is_empty() {
            return false;
        }
        let base_name = base
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default();
        if base_name.eq_ignore_ascii_case("versions") {
            return true;
        }
        if matching.len() >= 2 {
            return true;
        }
        matching.iter().any(|child| {
            let child_base = format!("{base}{child}/");
            self.has_file(&child_base, &format!("{child}.jar"))
        })
    }

    /// Whether `instances/<id>/instance.cfg` exists under `base`
    /// (MultiMC / Prism launcher instance marker).
    fn has_mmc_instance(&self, base: &str) -> bool {
        self.files.iter().any(|path| {
            let Some(rest) = path.strip_prefix(base) else {
                return false;
            };
            let parts: Vec<&str> = rest.split('/').collect();
            parts.len() == 3
                && parts[0] == "instances"
                && parts[2] == "instance.cfg"
                && !parts[1].is_empty()
        })
    }

    /// Whether `mods/` contains at least one `.jar` under `base`.
    fn has_mods_jar(&self, base: &str) -> bool {
        self.files.iter().any(|path| {
            let Some(rest) = path.strip_prefix(base) else {
                return false;
            };
            let parts: Vec<&str> = rest.split('/').collect();
            parts.len() == 2
                && parts[0] == "mods"
                && parts[1].to_lowercase().ends_with(".jar")
        })
    }

    /// Whether at least two direct child folders under `base` each look like
    /// an instance folder (root jar+json, `versions/<id>` or `mods/*.jar`).
    /// PCL-style launcher folders bundle several version folders this way;
    /// classification reports the common parent and the instance scan
    /// enumerates the children.
    fn has_multiple_instance_children(&self, base: &str) -> bool {
        let mut count = 0;
        for child in self.child_folders(base) {
            let child_base = format!("{base}{child}/");
            if self.has_version_json(&child_base)
                || self.has_direct_version_json(&child_base)
                || self.has_mods_jar(&child_base)
                || self.has_root_jar_and_json(&child_base)
            {
                count += 1;
                if count >= 2 {
                    return true;
                }
            }
        }
        false
    }

    /// Whether `shaders/` below `base` contains at least one entry (file or
    /// folder, of any kind). An empty `shaders/` directory is not a shader
    /// pack.
    fn has_populated_shaders_dir(&self, base: &str) -> bool {
        let prefix = format!("{base}shaders/");
        self.files.iter().any(|path| path.starts_with(&prefix))
            || self.dirs.iter().any(|dir| dir.starts_with(&prefix))
    }

    /// Whether `base` holds both a `.jar` and a `.json` file (modded
    /// instance root marker).
    fn has_root_jar_and_json(&self, base: &str) -> bool {
        let mut has_jar = false;
        let mut has_json = false;
        for path in &self.files {
            let Some(rest) = path.strip_prefix(base) else {
                continue;
            };
            if rest.contains('/') {
                continue;
            }
            if rest.to_lowercase().ends_with(".jar") {
                has_jar = true;
            }
            if rest.to_lowercase().ends_with(".json") {
                has_json = true;
            }
        }
        has_jar && has_json
    }

    /// Direct child directories under `base`, without the trailing slash.
    fn child_folders(&self, base: &str) -> Vec<String> {
        let mut children: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<&str> =
            std::collections::HashSet::new();
        for path in &self.dirs {
            let Some(rest) = path.strip_prefix(base) else {
                continue;
            };
            let first = rest.split('/').next().unwrap_or_default();
            if !first.is_empty() && !is_noise_entry(first) && seen.insert(first)
            {
                children.push(first.to_string());
            }
        }
        for path in &self.files {
            let Some(rest) = path.strip_prefix(base) else {
                continue;
            };
            let Some((first, _)) = rest.split_once('/') else {
                continue;
            };
            if !first.is_empty() && !is_noise_entry(first) && seen.insert(first)
            {
                children.push(first.to_string());
            }
        }
        children.sort();
        children
    }

    /// Whether `base` has a direct child folder with the given name.
    fn has_child_folder(&self, base: &str, name: &str) -> bool {
        self.child_folders(base)
            .iter()
            .any(|child| child.eq_ignore_ascii_case(name))
    }

    /// Direct child files under `base` that are themselves ZIP-family
    /// archives (.zip / .mrpack).
    fn nested_zip_files(&self, base: &str) -> Vec<String> {
        let mut zips: Vec<String> = Vec::new();
        for path in &self.files {
            let Some(rest) = path.strip_prefix(base) else {
                continue;
            };
            if rest.contains('/') {
                continue;
            }
            if rest.to_lowercase().ends_with(".zip")
                || rest.to_lowercase().ends_with(".mrpack")
            {
                zips.push(rest.to_string());
            }
        }
        zips.sort();
        zips
    }
}

/// Whether an entry-name segment is operating-system noise that must not be
/// recursed into (macOS resource forks, VCS metadata, thumbnails).
fn is_noise_entry(name: &str) -> bool {
    NOISE_ENTRY_NAMES
        .iter()
        .any(|noise| name.eq_ignore_ascii_case(noise))
}

/// Direct child folders and ZIP archives under `path`, sorted for stable
/// classification and without symlinks or OS noise entries.
fn sorted_folder_children(path: &Path) -> Vec<PathBuf> {
    let mut children = Vec::new();
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if is_noise_entry(&name) {
                continue;
            }
            let child = entry.path();
            if file_type.is_dir() || is_zip_path(&child) {
                children.push(child);
            }
        }
    }
    children.sort();
    children
}

fn resource_pack_candidates(entries: &ZipEntrySet, base: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    if entries.has_child_folder(base, "data") {
        candidates.push("data_pack".to_string());
    }
    if entries.has_child_folder(base, "assets") {
        candidates.push("resource_pack".to_string());
    }
    candidates
}

/// Whether the filesystem holding `dir` has at least `required` bytes free.
/// Returns `true` when the free space cannot be determined so the hard size
/// cap still applies as a fallback.
fn temp_dir_has_space(dir: &Path, required: u64) -> bool {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut best: Option<(usize, u64)> = None;
    for disk in disks.list() {
        let mount = disk.mount_point();
        if dir.starts_with(mount)
            && best.is_none_or(|(len, _)| mount.as_os_str().len() > len)
        {
            best = Some((mount.as_os_str().len(), disk.available_space()));
        }
    }
    match best {
        Some((_, available)) => available >= required,
        None => true,
    }
}

/// Classify a ZIP file with the layered, recursive flow:
///
/// 1. Modpack manifests (modrinth.index.json, manifest.json, ...)
/// 2. Compressed instances / `.minecraft` (versions, mods, launcher cfg)
/// 3. Resource packs (`pack.mcmeta`)
/// 4. Shader packs (`shaders/`)
/// 5. World saves (`level.dat`)
/// 6. When none match, descend into nested folders and nested ZIP files,
///    treating each as the new root, up to [`MAX_ZIP_NESTING_DEPTH`] levels.
///
/// Everything runs on entry names; nothing is extracted during
/// classification.
fn classify_zip_path(
    path: &Path,
    allow_nested_unpack: bool,
    cancellation: Option<&CancellationToken>,
) -> DroppedItemType {
    if is_cancelled(cancellation) {
        return cancelled_result();
    }
    let mut nested_unpack_bytes: u64 = 0;
    classify_zip_file_at_depth(
        path,
        0,
        allow_nested_unpack,
        &mut nested_unpack_bytes,
        cancellation,
    )
}

fn classify_zip_file_at_depth(
    path: &Path,
    depth: u32,
    allow_nested_unpack: bool,
    nested_unpack_bytes: &mut u64,
    cancellation: Option<&CancellationToken>,
) -> DroppedItemType {
    if is_cancelled(cancellation) {
        return cancelled_result();
    }
    let Ok(file) = std::fs::File::open(path) else {
        return DroppedItemType::Unknown {
            reason: "Cannot open ZIP file".to_string(),
        };
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return DroppedItemType::Unknown {
            reason: "File is not a valid ZIP archive".to_string(),
        };
    };
    let entry_set = match ZipEntrySet::from_archive_with_cancellation(
        &mut archive,
        cancellation,
    ) {
        Ok(set) => set,
        Err(reason) => return DroppedItemType::Unknown { reason },
    };
    classify_zip_entries(
        path,
        &mut archive,
        &entry_set,
        depth,
        "",
        allow_nested_unpack,
        nested_unpack_bytes,
        cancellation,
    )
}

/// Recursive core of ZIP classification for one virtual root (`base`).
fn classify_zip_entries<R: std::io::Read + std::io::Seek>(
    result_path: &Path,
    archive: &mut zip::ZipArchive<R>,
    entries: &ZipEntrySet,
    depth: u32,
    base: &str,
    allow_nested_unpack: bool,
    nested_unpack_bytes: &mut u64,
    cancellation: Option<&CancellationToken>,
) -> DroppedItemType {
    if is_cancelled(cancellation) {
        return cancelled_result();
    }
    // 1. Modpack manifests. Checked first because a modpack can carry
    //    overrides that look like resource packs, shader packs or worlds.
    match crate::api::pack::detect::detect_at_base(
        archive,
        &entries.names,
        base,
    ) {
        Ok(Some(detected)) => {
            let is_pack = matches!(
                detected.format,
                LocalPackFormat::Mrpack
                    | LocalPackFormat::CurseForge
                    | LocalPackFormat::Mcbbs
                    | LocalPackFormat::Hmcl
                    | LocalPackFormat::MmcExport
                    | LocalPackFormat::LauncherBundled
            );
            if is_pack {
                tracing::debug!(
                    "ZIP classify: modpack {:?} at base {:?} — {}",
                    detected.format,
                    base,
                    result_path.display()
                );
                return DroppedItemType::Modpack {
                    file_path: result_path.to_path_buf(),
                };
            }
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(
                "ZIP classify: pack detection failed at base {:?}: {error}",
                base
            );
        }
    }

    // 2. Compressed instances / `.minecraft`. Checked before the content
    //    markers because instances embed resource packs, shader packs and
    //    worlds.
    let launcher_type = if entries.has_axolotl_config(base)
        || entries.has_direct_axolotl_config(base)
    {
        Some(ImportLauncherType::Axolotl)
    } else if entries.has_file(base, "multimc.cfg") {
        Some(ImportLauncherType::MultiMC)
    } else if entries.has_file(base, "prismlauncher.cfg") {
        Some(ImportLauncherType::PrismLauncher)
    } else if entries.has_mmc_instance(base)
        || entries.has_file(base, "instance.cfg")
    {
        Some(ImportLauncherType::MultiMC)
    } else if entries.has_version_json(base)
        || entries.has_direct_version_json(base)
        || entries.has_mods_jar(base)
        || entries.has_root_jar_and_json(base)
        || entries.has_multiple_instance_children(base)
    {
        Some(ImportLauncherType::Generic)
    } else if entries.has_file(base, ".hmcl/config/launcher-settings.json") {
        Some(ImportLauncherType::HMCL)
    } else {
        None
    };
    if let Some(launcher_type) = launcher_type {
        tracing::debug!(
            "ZIP classify: instance {:?} at base {:?} — {}",
            launcher_type,
            base,
            result_path.display()
        );
        return DroppedItemType::Launcher {
            launcher_type,
            base_path: result_path.to_path_buf(),
            inner_base: zip_inner_base(base),
        };
    }

    // 3-5. Content markers, matched by name and structure at the current
    //    root: `pack.mcmeta`, a populated `shaders/` folder, or `level.dat`.
    //    Nested folders are handled by the recursion below, which re-runs
    //    this whole flow with the folder as the new root.
    if entries.has_file(base, "pack.mcmeta") {
        let candidates = resource_pack_candidates(entries, base);
        if !candidates.is_empty() {
            tracing::debug!(
                "ZIP classify: pack.mcmeta with candidates {candidates:?} at base {:?} — {}",
                base,
                result_path.display()
            );
            return DroppedItemType::ResourcePack {
                file_path: result_path.to_path_buf(),
                candidates,
                inner_base: zip_inner_base(base),
            };
        }
    }
    if entries.has_populated_shaders_dir(base) {
        tracing::debug!(
            "ZIP classify: shaders/ → ShaderPack at base {:?} — {}",
            base,
            result_path.display()
        );
        return DroppedItemType::ShaderPack {
            file_path: result_path.to_path_buf(),
            inner_base: zip_inner_base(base),
        };
    }
    if entries.has_file(base, "level.dat") {
        tracing::debug!(
            "ZIP classify: level.dat → WorldSave at base {:?} — {}",
            base,
            result_path.display()
        );
        return DroppedItemType::WorldSave {
            file_path: result_path.to_path_buf(),
            inner_base: zip_inner_base(base),
        };
    }

    // 6. Recurse into nested folders and nested ZIP files.
    if depth < MAX_ZIP_NESTING_DEPTH {
        for child in entries.child_folders(base) {
            if is_cancelled(cancellation) {
                return cancelled_result();
            }
            if child == "__MACOSX" {
                continue;
            }
            let child_base = format!("{base}{child}/");
            let result = classify_zip_entries(
                result_path,
                archive,
                entries,
                depth + 1,
                &child_base,
                allow_nested_unpack,
                nested_unpack_bytes,
                cancellation,
            );
            if !matches!(result, DroppedItemType::Unknown { .. }) {
                return result;
            }
        }
        for nested in entries.nested_zip_files(base) {
            if is_cancelled(cancellation) {
                return cancelled_result();
            }
            if !allow_nested_unpack {
                // First pass: report the nested archive instead of unpacking
                // it; the caller confirms with the user before staging.
                if let Some(size) = nested_zip_uncompressed_size(
                    archive,
                    &format!("{base}{nested}"),
                ) {
                    *nested_unpack_bytes += size;
                }
                continue;
            }
            let result = classify_nested_zip(
                result_path,
                archive,
                depth + 1,
                &format!("{base}{nested}"),
                allow_nested_unpack,
                cancellation,
            );
            if !matches!(result, DroppedItemType::Unknown { .. }) {
                return result;
            }
        }
    } else {
        tracing::debug!(
            "ZIP classify: nesting depth limit reached at base={base:?} — {}",
            result_path.display()
        );
        return DroppedItemType::Unknown {
            reason: format!(
                "Archive nesting is too deep to analyze (limit {MAX_ZIP_NESTING_DEPTH} levels)"
            ),
        };
    }

    tracing::debug!(
        "ZIP classify: inconclusive at depth={depth} base={base:?} — {}",
        result_path.display()
    );
    if *nested_unpack_bytes > 0 {
        DroppedItemType::Unknown {
            reason: format!(
                "Archive contains nested archives that must be unpacked to analyze (total {nested_unpack_bytes} bytes)"
            ),
        }
    } else if entries.has_encrypted {
        DroppedItemType::Unknown {
            reason:
                "Archive contains encrypted files and cannot be fully analyzed"
                    .to_string(),
        }
    } else {
        DroppedItemType::Unknown {
            reason:
                "ZIP archive requires extraction to determine content type (large archives may take a while)"
                    .to_string(),
        }
    }
}

/// Reads a nested ZIP entry once into a temporary file and classifies it
/// with the same layered flow, remapping the result path back to the outer
/// archive so no temp path leaks into the classification result.
fn classify_nested_zip<R: std::io::Read + std::io::Seek>(
    result_path: &Path,
    archive: &mut zip::ZipArchive<R>,
    depth: u32,
    entry_path: &str,
    allow_nested_unpack: bool,
    cancellation: Option<&CancellationToken>,
) -> DroppedItemType {
    if is_cancelled(cancellation) {
        return cancelled_result();
    }
    let Some(index) =
        crate::api::pack::detect::find_entry_index(archive, entry_path)
            .ok()
            .flatten()
    else {
        return DroppedItemType::Unknown {
            reason: format!("Cannot find nested archive entry {entry_path}"),
        };
    };
    let Ok(mut entry) = archive.by_index(index) else {
        return DroppedItemType::Unknown {
            reason: format!("Cannot read nested archive entry {entry_path}"),
        };
    };
    let temp_dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(error) => {
            return DroppedItemType::Unknown {
                reason: format!(
                    "Failed to create temporary directory: {error}"
                ),
            };
        }
    };
    if !temp_dir_has_space(temp_dir.path(), entry.size()) {
        return DroppedItemType::Unknown {
            reason: format!(
                "Not enough free disk space to stage nested archive {entry_path}"
            ),
        };
    }
    let nested_path = temp_dir.path().join("nested.zip");
    let mut output = match std::fs::File::create(&nested_path) {
        Ok(output) => output,
        Err(error) => {
            return DroppedItemType::Unknown {
                reason: format!(
                    "Failed to stage nested archive {entry_path}: {error}"
                ),
            };
        }
    };
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        if is_cancelled(cancellation) {
            return cancelled_result();
        }
        let read = match std::io::Read::read(&mut entry, &mut buffer) {
            Ok(read) => read,
            Err(error) => {
                return DroppedItemType::Unknown {
                    reason: format!(
                        "Failed to stage nested archive {entry_path}: {error}"
                    ),
                };
            }
        };
        if read == 0 {
            break;
        }
        if let Err(error) =
            std::io::Write::write_all(&mut output, &buffer[..read])
        {
            return DroppedItemType::Unknown {
                reason: format!(
                    "Failed to stage nested archive {entry_path}: {error}"
                ),
            };
        }
    }

    if is_cancelled(cancellation) {
        return cancelled_result();
    }

    let mut nested_unpack_bytes: u64 = 0;
    let result = classify_zip_file_at_depth(
        &nested_path,
        depth,
        allow_nested_unpack,
        &mut nested_unpack_bytes,
        cancellation,
    );
    let remap_inner = |inner_base: Option<String>| -> Option<String> {
        let combined = match inner_base {
            Some(inner) => format!("{entry_path}/{inner}"),
            None => entry_path.trim_end_matches('/').to_string(),
        };
        sanitize_inner_base(&combined)
    };
    match result {
        DroppedItemType::Unknown { reason } => DroppedItemType::Unknown {
            reason: format!("nested archive {entry_path}: {reason}"),
        },
        DroppedItemType::WorldSave { inner_base, .. } => {
            DroppedItemType::WorldSave {
                file_path: result_path.to_path_buf(),
                inner_base: remap_inner(inner_base),
            }
        }
        DroppedItemType::ResourcePack {
            candidates,
            inner_base,
            ..
        } => DroppedItemType::ResourcePack {
            file_path: result_path.to_path_buf(),
            candidates,
            inner_base: remap_inner(inner_base),
        },
        DroppedItemType::ShaderPack { inner_base, .. } => {
            DroppedItemType::ShaderPack {
                file_path: result_path.to_path_buf(),
                inner_base: remap_inner(inner_base),
            }
        }
        DroppedItemType::Modpack { .. } => DroppedItemType::Modpack {
            file_path: result_path.to_path_buf(),
        },
        DroppedItemType::Launcher { launcher_type, .. } => {
            DroppedItemType::Launcher {
                launcher_type,
                base_path: result_path.to_path_buf(),
                // The launcher lives inside a nested archive; the frontend
                // cannot scan it until that archive is extracted too, so no
                // inner base is advertised.
                inner_base: None,
            }
        }
        other => other,
    }
}

/// Uncompressed size of a nested archive entry, read from its metadata
/// without unpacking anything.
fn nested_zip_uncompressed_size<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    entry_path: &str,
) -> Option<u64> {
    let index = crate::api::pack::detect::find_entry_index(archive, entry_path)
        .ok()
        .flatten()?;
    archive.by_index_raw(index).ok().map(|entry| entry.size())
}

/// Extracts a ZIP archive to a temporary directory and classifies its contents
/// by examining the extracted files and folders.
///
/// This is a potentially **long-running** operation — the caller MUST first
/// confirm with the user before calling this function.
pub fn classify_zip_with_extraction(path: &Path) -> DroppedItemType {
    classify_zip_with_extraction_inner(path, None)
}

/// Extract and classify a ZIP while cooperating with a caller-controlled
/// cancellation token.
pub fn classify_zip_with_extraction_cancellable(
    path: &Path,
    cancellation: &CancellationToken,
) -> DroppedItemType {
    classify_zip_with_extraction_inner(path, Some(cancellation))
}

fn classify_zip_with_extraction_inner(
    path: &Path,
    cancellation: Option<&CancellationToken>,
) -> DroppedItemType {
    if is_cancelled(cancellation) {
        return cancelled_result();
    }
    let Ok(file) = std::fs::File::open(path) else {
        return DroppedItemType::Unknown {
            reason: "Cannot open ZIP file".to_string(),
        };
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return DroppedItemType::Unknown {
            reason: "File is not a valid ZIP archive".to_string(),
        };
    };
    let entry_set = match ZipEntrySet::from_archive_with_cancellation(
        &mut archive,
        cancellation,
    ) {
        Ok(set) => set,
        Err(reason) => return DroppedItemType::Unknown { reason },
    };

    let child_folders = entry_set.child_folders("");
    let root_files: Vec<&str> = entry_set
        .files
        .iter()
        .filter(|path| !path.contains('/'))
        .map(|path| path.as_str())
        .collect();
    let top_level_count = child_folders.len() + root_files.len();
    if top_level_count == 0 {
        return DroppedItemType::Unknown {
            reason: "Empty zip file".to_string(),
        };
    }
    if top_level_count > ZIP_TOP_LEVEL_LIMIT {
        return DroppedItemType::Unknown {
            reason: "ZIP archive has too many top-level entries".to_string(),
        };
    }

    // Create temporary directory for extraction.
    let temp_dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            return DroppedItemType::Unknown {
                reason: format!("Failed to create temporary directory: {e}"),
            };
        }
    };

    // Extract everything.
    if let Err(reason) = extract_all_with_cancellation(
        &mut archive,
        temp_dir.path(),
        cancellation,
    ) {
        return DroppedItemType::Unknown {
            reason: format!("ZIP extraction failed: {reason}"),
        };
    }

    tracing::debug!(
        "classify_zip_with_extraction: extracted {} top-level items for {}",
        top_level_count,
        path.display()
    );

    // Classify the extracted contents: a single top-level item is classified
    // directly, otherwise the extraction root is treated as a folder.
    if is_cancelled(cancellation) {
        return cancelled_result();
    }
    if child_folders.len() == 1 && root_files.is_empty() {
        classify_dropped_item_inner(
            &temp_dir.path().join(&child_folders[0]),
            0,
            true,
            cancellation,
        )
    } else if root_files.len() == 1 && child_folders.is_empty() {
        classify_dropped_item_inner(
            &temp_dir.path().join(root_files[0]),
            0,
            true,
            cancellation,
        )
    } else {
        classify_folder_content_with_cancellation(
            temp_dir.path(),
            true,
            cancellation,
        )
    }
    // temp_dir is dropped here, cleaning up the extracted files automatically.
}

fn validate_zip_extraction_budget<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<u64, String> {
    let mut total_bytes = 0u64;
    let mut entries = 0usize;
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(|error| {
            format!("Cannot inspect ZIP entry {index}: {error}")
        })?;
        if entry.name().is_empty() {
            continue;
        }
        entries += 1;
        if entries > MAX_ZIP_EXTRACTION_ENTRIES {
            return Err(format!(
                "ZIP contains more than {MAX_ZIP_EXTRACTION_ENTRIES} entries"
            ));
        }
        total_bytes =
            total_bytes.checked_add(entry.size()).ok_or_else(|| {
                "ZIP uncompressed size exceeds supported limits".to_string()
            })?;
        if total_bytes > MAX_ZIP_EXTRACTION_BYTES {
            return Err(format!(
                "ZIP uncompressed size exceeds {} GiB",
                MAX_ZIP_EXTRACTION_BYTES / (1024 * 1024 * 1024)
            ));
        }
    }
    Ok(total_bytes)
}

#[cfg(test)]
fn extract_all(
    archive: &mut zip::ZipArchive<std::fs::File>,
    base_dir: &Path,
) -> Result<(), String> {
    extract_all_with_cancellation(archive, base_dir, None)
}

fn extract_all_with_cancellation(
    archive: &mut zip::ZipArchive<std::fs::File>,
    base_dir: &Path,
    cancellation: Option<&CancellationToken>,
) -> Result<(), String> {
    if is_cancelled(cancellation) {
        return Err("Operation cancelled".to_string());
    }
    let total_bytes = validate_zip_extraction_budget(archive)?;
    if !temp_dir_has_space(base_dir, total_bytes) {
        return Err(format!(
            "Not enough free disk space to extract ZIP (requires {total_bytes} bytes)"
        ));
    }

    // First pass: collect entry metadata while the archive is mutable-borrowed.
    let entries: Vec<(usize, String, bool)> = (0..archive.len())
        .filter_map(|i| {
            let entry = archive.by_index_raw(i).ok()?;
            let name = entry.name().to_string();
            if name.is_empty() {
                None
            } else {
                Some((i, name.clone(), name.ends_with('/')))
            }
        })
        .collect();

    // Second pass: extract. The collect() above has released the mutable
    // borrow, so we can call by_index() here.
    for (index, name, is_dir) in &entries {
        if is_cancelled(cancellation) {
            return Err("Operation cancelled".to_string());
        }
        if archive
            .by_index_raw(*index)
            .ok()
            .is_some_and(|e| e.encrypted())
        {
            return Err(format!(
                "Encrypted ZIP entry cannot be extracted: {name}"
            ));
        }
        // Reject entries that would escape the extraction directory.
        let Some(safe_name) = sanitize_zip_entry_name(name) else {
            tracing::warn!(
                "extract_all: skipping unsafe ZIP entry '{}' (path traversal)",
                name
            );
            continue;
        };
        let out_path = base_dir.join(&safe_name);
        if *is_dir {
            std::fs::create_dir_all(&out_path).map_err(|error| {
                format!("Failed to create ZIP directory '{name}': {error}")
            })?;
        } else if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!("Failed to create ZIP parent for '{name}': {error}")
            })?;
            let mut reader = archive.by_index(*index).map_err(|error| {
                format!("Failed to read ZIP entry '{name}': {error}")
            })?;
            let mut writer =
                std::fs::File::create(&out_path).map_err(|error| {
                    format!("Failed to create extracted file '{name}': {error}")
                })?;
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                if is_cancelled(cancellation) {
                    return Err("Operation cancelled".to_string());
                }
                let read = reader.read(&mut buffer).map_err(|error| {
                    format!("Failed to extract ZIP entry '{name}': {error}")
                })?;
                if read == 0 {
                    break;
                }
                writer.write_all(&buffer[..read]).map_err(|error| {
                    format!("Failed to extract ZIP entry '{name}': {error}")
                })?;
            }
        }
    }
    Ok(())
}

/// Extract a ZIP archive into `dest_dir`, skipping unsafe entries (path
/// traversal, absolute paths, drive letters) and macOS resource forks.
///
/// Used by the app to materialize a compressed launcher folder before
/// scanning and importing instances. The caller owns the temporary
/// directory lifecycle.
pub fn extract_zip_to_dir(
    zip_path: &Path,
    dest_dir: &Path,
) -> Result<(), String> {
    extract_zip_to_dir_with_cancellation(zip_path, dest_dir, None)
}

/// Extract a ZIP into a directory while cooperating with cancellation.
pub fn extract_zip_to_dir_cancellable(
    zip_path: &Path,
    dest_dir: &Path,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    extract_zip_to_dir_with_cancellation(zip_path, dest_dir, Some(cancellation))
}

fn extract_zip_to_dir_with_cancellation(
    zip_path: &Path,
    dest_dir: &Path,
    cancellation: Option<&CancellationToken>,
) -> Result<(), String> {
    if is_cancelled(cancellation) {
        return Err("Operation cancelled".to_string());
    }
    let file = std::fs::File::open(zip_path).map_err(|e| {
        format!("Cannot open ZIP file '{}': {e}", zip_path.display())
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        format!("Invalid ZIP archive '{}': {e}", zip_path.display())
    })?;
    let total_bytes = validate_zip_extraction_budget(&mut archive)?;
    if !temp_dir_has_space(dest_dir, total_bytes) {
        return Err(format!(
            "Not enough free disk space to extract ZIP (requires {total_bytes} bytes)"
        ));
    }

    let entries: Vec<(usize, String, bool)> = (0..archive.len())
        .map(|i| {
            let entry = archive.by_index_raw(i).map_err(|error| {
                format!("Cannot inspect ZIP entry {i}: {error}")
            })?;
            let name = crate::api::pack::detect::decode_zip_entry_name(
                entry.name_raw(),
            )
            .replace('\\', "/");
            if name.is_empty() || name.starts_with("__MACOSX") {
                return Ok(None);
            }
            Ok(Some((i, name.clone(), name.ends_with('/'))))
        })
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .flatten()
        .collect();

    let mut skipped = 0usize;
    let mut extracted_entries = 0usize;
    let mut failed_entries = Vec::new();
    for (index, name, is_dir) in entries {
        if is_cancelled(cancellation) {
            return Err("Operation cancelled".to_string());
        }
        let encrypted = archive
            .by_index_raw(index)
            .map_err(|error| {
                format!("Cannot inspect ZIP entry '{name}': {error}")
            })?
            .encrypted();
        if encrypted {
            tracing::warn!(
                "extract_zip_to_dir: cannot extract encrypted ZIP entry '{name}'"
            );
            skipped += 1;
            failed_entries.push(name);
            continue;
        }
        let Some(safe_name) = sanitize_zip_entry_name(&name) else {
            tracing::warn!(
                "extract_zip_to_dir: skipping unsafe ZIP entry '{name}' (path traversal)"
            );
            skipped += 1;
            continue;
        };
        if safe_name.as_os_str().is_empty() {
            skipped += 1;
            continue;
        }
        let out_path = dest_dir.join(&safe_name);
        let result = if is_dir {
            std::fs::create_dir_all(&out_path).map(|_| ())
        } else if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).and_then(|_| {
                let mut reader =
                    archive.by_index(index).map_err(std::io::Error::other)?;
                let mut writer = std::fs::File::create(&out_path)?;
                let mut buffer = vec![0u8; 64 * 1024];
                loop {
                    if is_cancelled(cancellation) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Interrupted,
                            "operation cancelled",
                        ));
                    }
                    let read = reader.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    writer.write_all(&buffer[..read])?;
                }
                Ok(())
            })
        } else {
            Ok(())
        };
        if let Err(error) = result {
            tracing::warn!(
                "extract_zip_to_dir: skipping unextractable entry '{name}': {error}"
            );
            skipped += 1;
            failed_entries.push(name);
        } else {
            extracted_entries += 1;
        }
    }
    if skipped > 0 {
        tracing::warn!(
            "extract_zip_to_dir: skipped {skipped} of {} entries",
            archive.len()
        );
    }
    if !failed_entries.is_empty() {
        return Err(format!(
            "ZIP extraction failed for {} entr{}: {}",
            failed_entries.len(),
            if failed_entries.len() == 1 {
                "y"
            } else {
                "ies"
            },
            failed_entries.join(", ")
        ));
    }
    if !archive.is_empty() && extracted_entries == 0 {
        return Err(format!(
            "No ZIP entries could be extracted ({skipped} skipped)"
        ));
    }
    Ok(())
}

/// Normalize a ZIP entry name into a safe relative path that stays inside the
/// extraction directory. Returns `None` for absolute paths or entries
/// containing `..` (zip-slip protection).
fn sanitize_zip_entry_name(name: &str) -> Option<PathBuf> {
    // The ZIP spec mandates `/` separators, but tolerate backslashes from
    // Windows-authored archives.
    let normalized = name.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return None;
    }
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => safe.push(part),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return None,
        }
    }
    Some(safe)
}

// ─── Step 3: Launcher EXE ──────────────────────────────────────────────────

fn classify_launcher_exe(path: &Path) -> DroppedItemType {
    if let Some(parent) = path.parent() {
        match crate::api::pack::import::pe_info::folder_has_product_result(
            parent,
            "Plain Craft Launcher",
        ) {
            Ok(true) => {
                if crate::api::pack::import::config_exists() {
                    return DroppedItemType::Launcher {
                        launcher_type: ImportLauncherType::PCL2CE,
                        base_path: parent.to_path_buf(),
                        inner_base: None,
                    };
                }
                if crate::api::pack::import::read_pcl_registry().is_some() {
                    return DroppedItemType::Launcher {
                        launcher_type: ImportLauncherType::PCL2,
                        base_path: parent.to_path_buf(),
                        inner_base: None,
                    };
                }
                return DroppedItemType::Launcher {
                    launcher_type: ImportLauncherType::PCL2,
                    base_path: parent.to_path_buf(),
                    inner_base: None,
                };
            }
            Ok(false) => {}
            Err(_) => {}
        }

        match crate::api::pack::import::pe_info::folder_has_product_result(
            parent,
            "Hello Minecraft! Launcher",
        ) {
            Ok(true) => {
                return DroppedItemType::Launcher {
                    launcher_type: ImportLauncherType::HMCL,
                    base_path: parent.to_path_buf(),
                    inner_base: None,
                };
            }
            Ok(false) => {}
            Err(_) => {}
        }
    }

    DroppedItemType::Unknown {
        reason: format!("Unrecognised executable: {}", path.display()),
    }
}

// ─── Step 4: JAR file ──────────────────────────────────────────────────────

fn classify_jar(path: &Path) -> DroppedItemType {
    let manifest = read_jar_manifest(path);

    if let Some(ref mf) = manifest {
        // HMCL launcher JAR.
        if mf.main_class.as_deref() == Some("org.jackhuang.hmcl.Main") {
            if let Some(parent) = path.parent()
                && let Some(data_dir) =
                    crate::api::pack::import::hmcl_config::find_hmcl_data_dir(
                        parent,
                    )
            {
                return DroppedItemType::HmclLauncher {
                    launcher_dir: parent.to_path_buf(),
                    data_dir,
                };
            }
            // Found HMCL main class but no data dir — still classify as launcher.
            return DroppedItemType::Launcher {
                launcher_type: ImportLauncherType::HMCL,
                base_path: path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default(),
                inner_base: None,
            };
        }
    }

    // Otherwise, treat as a mod.
    DroppedItemType::Mod {
        file_path: path.to_path_buf(),
    }
}

// ─── Step 5: Folder classification ─────────────────────────────────────────

fn classify_folder(
    path: &Path,
    allow_nested_unpack: bool,
    cancellation: Option<&CancellationToken>,
) -> DroppedItemType {
    classify_folder_content_with_cancellation(
        path,
        allow_nested_unpack,
        cancellation,
    )
}

// ─── Step 6: File classification (non-JAR, non-EXE, non-ZIP) ───────────────

fn classify_file(path: &Path) -> DroppedItemType {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "litematic" | "schematic" => DroppedItemType::Litematic {
            file_path: path.to_path_buf(),
        },
        _ => DroppedItemType::Unknown {
            reason: format!("Unrecognised file type: {}", path.display()),
        },
    }
}

// ─── Step 7: Content-type detection for folders/extracted ZIPs ─────────────

fn classify_folder_content_with_cancellation(
    path: &Path,
    allow_nested_unpack: bool,
    cancellation: Option<&CancellationToken>,
) -> DroppedItemType {
    let mut nested_unpack_bytes: u64 = 0;
    classify_folder_content_inner(
        path,
        allow_nested_unpack,
        0,
        &mut nested_unpack_bytes,
        cancellation,
    )
}

fn classify_folder_content_inner(
    path: &Path,
    allow_nested_unpack: bool,
    depth: u32,
    nested_unpack_bytes: &mut u64,
    cancellation: Option<&CancellationToken>,
) -> DroppedItemType {
    if is_cancelled(cancellation) {
        return cancelled_result();
    }
    // Check launcher signatures in priority order.
    if is_axolotl_folder(path) {
        return DroppedItemType::Launcher {
            launcher_type: ImportLauncherType::Axolotl,
            base_path: path.to_path_buf(),
            inner_base: None,
        };
    }

    if path.join("multimc.cfg").exists() {
        return DroppedItemType::Launcher {
            launcher_type: ImportLauncherType::MultiMC,
            base_path: path.to_path_buf(),
            inner_base: None,
        };
    }

    if path.join("prismlauncher.cfg").exists() {
        return DroppedItemType::Launcher {
            launcher_type: ImportLauncherType::PrismLauncher,
            base_path: path.to_path_buf(),
            inner_base: None,
        };
    }

    // MultiMC/Prism: check for instances/<sub>/instance.cfg pattern.
    if let Ok(mut dir) = std::fs::read_dir(path.join("instances"))
        && dir.any(|e| {
            e.ok()
                .as_ref()
                .is_some_and(|e| e.path().join("instance.cfg").exists())
        })
    {
        // instance.cfg → MultiMC or Prism.
        return DroppedItemType::Launcher {
            launcher_type: ImportLauncherType::MultiMC,
            base_path: path.to_path_buf(),
            inner_base: None,
        };
    }

    // HMCL portable mode.
    let hmcl_config = path
        .join(".hmcl")
        .join("config")
        .join("launcher-settings.json");
    if hmcl_config.exists()
        && let Some(data_dir) =
            crate::api::pack::import::hmcl_config::find_hmcl_data_dir(path)
    {
        return DroppedItemType::HmclLauncher {
            launcher_dir: path.to_path_buf(),
            data_dir,
        };
    }

    // A directly dropped `.minecraft` directory is an external game
    // directory, even when it does not contain a complete launcher instance
    // manifest yet. Route it through the GameDir import flow so it is not
    // mistaken for a normal instance folder.
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(".minecraft"))
    {
        return DroppedItemType::Launcher {
            launcher_type: ImportLauncherType::Generic,
            base_path: path.to_path_buf(),
            inner_base: None,
        };
    }

    // Step 7: Content-type detection for folders.
    if let Some(result) = classify_world_save_folder(path) {
        return result;
    }
    if let Some(result) = classify_resource_pack_folder(path) {
        return result;
    }
    if let Some(result) = classify_shader_pack_folder(path) {
        return result;
    }
    if is_launcher_instance_folder(path) {
        return DroppedItemType::Launcher {
            launcher_type: ImportLauncherType::Generic,
            base_path: path.to_path_buf(),
            inner_base: None,
        };
    }

    if depth >= MAX_ZIP_NESTING_DEPTH {
        return DroppedItemType::Unknown {
            reason: format!(
                "Folder nesting is too deep to analyze (limit {MAX_ZIP_NESTING_DEPTH} levels)"
            ),
        };
    }

    let children = sorted_folder_children(path);

    for child in children {
        if is_cancelled(cancellation) {
            return cancelled_result();
        }
        let result = if child.is_dir() {
            classify_folder_content_inner(
                &child,
                allow_nested_unpack,
                depth + 1,
                nested_unpack_bytes,
                cancellation,
            )
        } else {
            classify_zip_file_at_depth(
                &child,
                depth + 1,
                allow_nested_unpack,
                nested_unpack_bytes,
                cancellation,
            )
        };
        if !matches!(result, DroppedItemType::Unknown { .. }) {
            return result;
        }
    }

    if *nested_unpack_bytes > 0 {
        return DroppedItemType::Unknown {
            reason: format!(
                "Archive contains nested archives that must be unpacked to analyze (total {nested_unpack_bytes} bytes)"
            ),
        };
    }

    DroppedItemType::Unknown {
        reason: format!(
            "Unrecognised content: {}",
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string())
        ),
    }
}

/// Classify a folder as a world save when it contains a `level.dat` file.
fn classify_world_save_folder(path: &Path) -> Option<DroppedItemType> {
    path.join("level.dat")
        .is_file()
        .then(|| DroppedItemType::WorldSave {
            file_path: path.to_path_buf(),
            inner_base: None,
        })
}

/// Classify a folder as a resource pack when it contains a `pack.mcmeta`.
fn classify_resource_pack_folder(path: &Path) -> Option<DroppedItemType> {
    if !path.join("pack.mcmeta").is_file() {
        return None;
    }
    let mut candidates = Vec::new();
    if has_child_dir(path, "data") {
        candidates.push("data_pack".to_string());
    }
    if has_child_dir(path, "assets") {
        candidates.push("resource_pack".to_string());
    }
    (!candidates.is_empty()).then(|| DroppedItemType::ResourcePack {
        file_path: path.to_path_buf(),
        candidates,
        inner_base: None,
    })
}

/// Whether a direct child directory with the given name exists.
fn has_child_dir(path: &Path, name: &str) -> bool {
    std::fs::read_dir(path).ok().is_some_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(name)
                && entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
        })
    })
}

/// Classify a folder as a shader pack when `shaders/` exists and contains at
/// least one entry (of any kind).
fn classify_shader_pack_folder(path: &Path) -> Option<DroppedItemType> {
    let shaders = path.join("shaders");
    if !shaders.is_dir() {
        return None;
    }
    let has_any_entry = std::fs::read_dir(&shaders).ok()?.next().is_some();
    has_any_entry.then(|| DroppedItemType::ShaderPack {
        file_path: path.to_path_buf(),
        inner_base: None,
    })
}

/// Classify an input and attach any alternative content candidates found in
/// the same folder / archive tree. The primary hit is always first.
pub fn classify_dropped_item_with_candidates(
    path: &Path,
    allow_nested_unpack: bool,
) -> DroppedItemType {
    classify_dropped_item_with_candidates_inner(path, allow_nested_unpack, None)
}

/// Classify an input and collect alternative content candidates while
/// cooperating with cancellation.
pub fn classify_dropped_item_with_candidates_cancellable(
    path: &Path,
    allow_nested_unpack: bool,
    cancellation: &CancellationToken,
) -> DroppedItemType {
    classify_dropped_item_with_candidates_inner(
        path,
        allow_nested_unpack,
        Some(cancellation),
    )
}

fn classify_dropped_item_with_candidates_inner(
    path: &Path,
    allow_nested_unpack: bool,
    cancellation: Option<&CancellationToken>,
) -> DroppedItemType {
    let primary =
        classify_dropped_item_inner(path, 0, allow_nested_unpack, cancellation);
    if matches!(
        primary,
        DroppedItemType::Launcher { .. } | DroppedItemType::HmclLauncher { .. }
    ) {
        return primary;
    }
    let mut alternatives =
        collect_content_candidates(path, allow_nested_unpack, cancellation);
    if matches!(primary, DroppedItemType::Unknown { .. })
        && alternatives.is_empty()
    {
        return primary;
    }

    let mut all = Vec::new();
    let primary_path = primary_path_of(&primary);
    let primary_inner_base = relative_to(path, &primary_path);
    if let Some(primary_candidate) =
        candidate_from_result(&primary, primary_inner_base)
    {
        all.push(primary_candidate);
    }
    for candidate in alternatives.drain(..) {
        if !all
            .iter()
            .any(|existing| same_candidate(existing, &candidate))
        {
            all.push(candidate);
        }
    }

    if all.len() <= 1 && !matches!(primary, DroppedItemType::Unknown { .. }) {
        return primary;
    }

    let has_primary =
        !matches!(primary, DroppedItemType::Unknown { .. }) && !all.is_empty();
    let primary_candidate = has_primary.then(|| all.remove(0));
    all.sort_by(|a, b| {
        candidate_priority(b)
            .cmp(&candidate_priority(a))
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    if let Some(primary_candidate) = primary_candidate {
        all.insert(0, primary_candidate);
    }

    DroppedItemType::Multiple {
        file_path: path.to_path_buf(),
        candidates: all,
    }
}

fn primary_path_of(item: &DroppedItemType) -> PathBuf {
    match item {
        DroppedItemType::Launcher { base_path, .. } => base_path.clone(),
        DroppedItemType::HmclLauncher { launcher_dir, .. } => {
            launcher_dir.clone()
        }
        DroppedItemType::Mod { file_path } => file_path.clone(),
        DroppedItemType::Litematic { file_path } => file_path.clone(),
        DroppedItemType::ResourcePack { file_path, .. } => file_path.clone(),
        DroppedItemType::ShaderPack { file_path, .. } => file_path.clone(),
        DroppedItemType::WorldSave { file_path, .. } => file_path.clone(),
        DroppedItemType::Modpack { file_path } => file_path.clone(),
        DroppedItemType::ShortcutResolved { resolved_to, .. } => {
            primary_path_of(resolved_to)
        }
        DroppedItemType::Unknown { .. } | DroppedItemType::Multiple { .. } => {
            PathBuf::new()
        }
    }
}

fn collect_content_candidates(
    path: &Path,
    allow_nested_unpack: bool,
    cancellation: Option<&CancellationToken>,
) -> Vec<DroppedCandidate> {
    if path.is_dir() {
        let mut candidates = Vec::new();
        collect_folder_candidates(
            path,
            allow_nested_unpack,
            path,
            0,
            &mut candidates,
            cancellation,
        );
        candidates
    } else if is_zip_path(path) {
        collect_zip_candidates(path, allow_nested_unpack, cancellation)
    } else {
        Vec::new()
    }
}

fn collect_folder_candidates(
    path: &Path,
    allow_nested_unpack: bool,
    root: &Path,
    depth: u32,
    out: &mut Vec<DroppedCandidate>,
    cancellation: Option<&CancellationToken>,
) {
    if depth >= MAX_ZIP_NESTING_DEPTH || is_cancelled(cancellation) {
        return;
    }

    let inner_base = relative_to(root, path);
    if let Some(result) = classify_world_save_folder(path) {
        push_candidate(out, candidate_from_result(&result, inner_base.clone()));
    }
    if let Some(result) = classify_resource_pack_folder(path) {
        push_candidate(out, candidate_from_result(&result, inner_base.clone()));
    }
    if let Some(result) = classify_shader_pack_folder(path) {
        push_candidate(out, candidate_from_result(&result, inner_base));
    }

    let children = sorted_folder_children(path);

    for child in children {
        if child.is_dir() {
            collect_folder_candidates(
                &child,
                allow_nested_unpack,
                root,
                depth + 1,
                out,
                cancellation,
            );
        } else {
            let result =
                classify_zip_path(&child, allow_nested_unpack, cancellation);
            let child_base = relative_to(root, &child);
            push_candidate(out, candidate_from_result(&result, child_base));
        }
    }
}

fn collect_zip_candidates(
    path: &Path,
    allow_nested_unpack: bool,
    cancellation: Option<&CancellationToken>,
) -> Vec<DroppedCandidate> {
    if is_cancelled(cancellation) {
        return Vec::new();
    }
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return Vec::new();
    };
    let Ok(entries) =
        ZipEntrySet::from_archive_with_cancellation(&mut archive, cancellation)
    else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    collect_zip_candidates_at(
        path,
        &mut archive,
        &entries,
        "",
        0,
        allow_nested_unpack,
        &mut candidates,
        cancellation,
    );
    candidates
}

fn collect_zip_candidates_at<R: std::io::Read + std::io::Seek>(
    result_path: &Path,
    archive: &mut zip::ZipArchive<R>,
    entries: &ZipEntrySet,
    base: &str,
    depth: u32,
    allow_nested_unpack: bool,
    out: &mut Vec<DroppedCandidate>,
    cancellation: Option<&CancellationToken>,
) {
    if depth >= MAX_ZIP_NESTING_DEPTH || is_cancelled(cancellation) {
        return;
    }

    let inner_base = zip_inner_base(base);
    if entries.has_file(base, "pack.mcmeta") {
        let candidates = resource_pack_candidates(entries, base);
        if !candidates.is_empty() {
            push_candidate(
                out,
                Some(DroppedCandidate {
                    item_type: "resource_pack".to_string(),
                    file_path: result_path.to_path_buf(),
                    inner_base: inner_base.clone(),
                    candidates,
                }),
            );
        }
    }
    if entries.has_populated_shaders_dir(base) {
        push_candidate(
            out,
            Some(DroppedCandidate {
                item_type: "shader_pack".to_string(),
                file_path: result_path.to_path_buf(),
                inner_base: inner_base.clone(),
                candidates: Vec::new(),
            }),
        );
    }
    if entries.has_file(base, "level.dat") {
        push_candidate(
            out,
            Some(DroppedCandidate {
                item_type: "world_save".to_string(),
                file_path: result_path.to_path_buf(),
                inner_base,
                candidates: Vec::new(),
            }),
        );
    }

    for child in entries.child_folders(base) {
        if is_cancelled(cancellation) {
            return;
        }
        if is_noise_entry(&child) {
            continue;
        }
        collect_zip_candidates_at(
            result_path,
            archive,
            entries,
            &format!("{base}{child}/"),
            depth + 1,
            allow_nested_unpack,
            out,
            cancellation,
        );
    }

    if allow_nested_unpack {
        for nested in entries.nested_zip_files(base) {
            if is_cancelled(cancellation) {
                return;
            }
            let result = classify_nested_zip(
                result_path,
                archive,
                depth + 1,
                &format!("{base}{nested}"),
                true,
                cancellation,
            );
            push_candidate(out, candidate_from_result(&result, None));
        }
    }
}

fn candidate_from_result(
    result: &DroppedItemType,
    outer_inner_base: Option<String>,
) -> Option<DroppedCandidate> {
    let (item_type, file_path, candidates, inner_base) = match result {
        DroppedItemType::Launcher {
            base_path,
            inner_base,
            ..
        } => (
            "launcher".to_string(),
            base_path.clone(),
            Vec::new(),
            inner_base.clone(),
        ),
        DroppedItemType::HmclLauncher { launcher_dir, .. } => (
            "hmcl_launcher".to_string(),
            launcher_dir.clone(),
            Vec::new(),
            None,
        ),
        DroppedItemType::Mod { file_path } => {
            ("mod".to_string(), file_path.clone(), Vec::new(), None)
        }
        DroppedItemType::Litematic { file_path } => {
            ("litematic".to_string(), file_path.clone(), Vec::new(), None)
        }
        DroppedItemType::ResourcePack {
            file_path,
            candidates,
            inner_base,
        } => (
            "resource_pack".to_string(),
            file_path.clone(),
            candidates.clone(),
            inner_base.clone(),
        ),
        DroppedItemType::ShaderPack {
            file_path,
            inner_base,
        } => (
            "shader_pack".to_string(),
            file_path.clone(),
            Vec::new(),
            inner_base.clone(),
        ),
        DroppedItemType::WorldSave {
            file_path,
            inner_base,
        } => (
            "world_save".to_string(),
            file_path.clone(),
            Vec::new(),
            inner_base.clone(),
        ),
        DroppedItemType::Modpack { file_path } => {
            ("modpack".to_string(), file_path.clone(), Vec::new(), None)
        }
        DroppedItemType::ShortcutResolved { resolved_to, .. } => {
            return candidate_from_result(resolved_to, outer_inner_base);
        }
        DroppedItemType::Unknown { .. } | DroppedItemType::Multiple { .. } => {
            return None;
        }
    };

    Some(DroppedCandidate {
        item_type,
        file_path,
        inner_base: merge_inner_base(outer_inner_base, inner_base),
        candidates,
    })
}

fn merge_inner_base(
    outer: Option<String>,
    inner: Option<String>,
) -> Option<String> {
    match (outer, inner) {
        (Some(outer), Some(inner)) => Some(format!("{outer}/{inner}")),
        (Some(outer), None) => Some(outer),
        (None, inner) => inner,
    }
}

fn push_candidate(
    out: &mut Vec<DroppedCandidate>,
    candidate: Option<DroppedCandidate>,
) {
    let Some(candidate) = candidate else {
        return;
    };
    if candidate.item_type == "unknown"
        || out
            .iter()
            .any(|existing| same_candidate(existing, &candidate))
        || out.len() >= 8
    {
        return;
    }
    out.push(candidate);
}

fn same_candidate(a: &DroppedCandidate, b: &DroppedCandidate) -> bool {
    a.item_type == b.item_type
        && a.file_path == b.file_path
        && a.inner_base == b.inner_base
}

fn candidate_priority(candidate: &DroppedCandidate) -> i32 {
    let base = match candidate.item_type.as_str() {
        "modpack" => 100,
        "launcher" | "hmcl_launcher" => 90,
        "world_save" => 70,
        "data_pack" => 65,
        "resource_pack" => 60,
        "shader_pack" => 55,
        "litematic" => 50,
        _ => 0,
    };
    let depth = candidate
        .inner_base
        .as_deref()
        .map(|base| base.split('/').count() as i32)
        .unwrap_or(0);
    base - depth * 5
}

fn relative_to(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .filter(|rel| !rel.is_empty())
}

fn zip_inner_base(base: &str) -> Option<String> {
    (!base.is_empty())
        .then(|| base.trim_end_matches('/'))
        .and_then(sanitize_inner_base)
}

fn sanitize_inner_base(base: &str) -> Option<String> {
    let normalized = base.replace('\\', "/");
    let path = std::path::Path::new(&normalized);
    if path.is_absolute() {
        return None;
    }

    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => {
                parts.push(part.to_string_lossy().into_owned());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return None,
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Detect launcher instance markers:
/// - `versions/<id>/<id>.json` pattern (vanilla launcher instance)
/// - a root `.jar` + `.json` pair (modded instance)
/// - `.jar` files in `mods/` (bare instance folder)
fn is_launcher_instance_folder(path: &Path) -> bool {
    has_version_json(path)
        || has_direct_version_json(path)
        || (has_root_jar(path) && has_root_json(path))
        || has_mods_jar(path)
}

/// Whether `versions/<id>/<id>.json` exists for any subdirectory of `versions/`.
fn has_version_json(path: &Path) -> bool {
    let versions_dir = path.join("versions");
    if !versions_dir.is_dir() {
        return false;
    }
    match std::fs::read_dir(&versions_dir) {
        Ok(mut dir) => dir.any(|e| {
            e.ok().is_some_and(|entry| {
                let p = entry.path();
                let Some(id) = p.file_name().and_then(|n| n.to_str()) else {
                    return false;
                };
                let json_path = p.join(format!("{id}.json"));
                let exists = p.is_dir() && json_path.exists();
                tracing::debug!(
                    "classify_folder_content: versions subdir={} json={} exists={}",
                    id,
                    json_path.display(),
                    exists
                );
                exists
            })
        }),
        Err(e) => {
            tracing::debug!(
                "classify_folder_content: versions_dir={} read_dir_err={}",
                versions_dir.display(),
                e
            );
            false
        }
    }
}

/// Whether direct child folders of `path` look like version folders
/// (`<id>/<id>.json`). A folder named `versions`, at least two matching
/// children, or a matching child with the same-name `.jar` makes the
/// classification specific enough to avoid false positives.
fn has_direct_version_json(path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    let mut matching: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let child = entry.path();
        if !child.is_dir() {
            continue;
        }
        let Some(id) = child.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if child.join(format!("{id}.json")).is_file() {
            matching.push(child);
        }
    }
    if matching.is_empty() {
        return false;
    }
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("versions"))
    {
        return true;
    }
    if matching.len() >= 2 {
        return true;
    }
    matching.iter().any(|child| {
        child
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|id| child.join(format!("{id}.jar")).is_file())
    })
}

/// Whether `path` is an Axolotl instance root or a container whose direct
/// child folders are Axolotl instances.
fn is_axolotl_folder(path: &Path) -> bool {
    path.join("axolotl_config.json").is_file()
        || has_direct_axolotl_config(path)
}

/// Whether a direct child folder of `path` has `axolotl_config.json`.
fn has_direct_axolotl_config(path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let child = entry.path();
        child.is_dir() && child.join("axolotl_config.json").is_file()
    })
}

/// Whether the directory contains a file with the given extension.
fn dir_has_extension(dir: &Path, extension: &str, label: &str) -> bool {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries.flatten().any(|entry| {
            let p = entry.path();
            if !p.is_file() {
                return false;
            }
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let is_match = ext.eq_ignore_ascii_case(extension);
            tracing::debug!(
                "classify_folder_content: {label} path={} file={} ext={} is_match={}",
                dir.display(),
                p.display(),
                ext,
                is_match
            );
            is_match
        }),
        Err(e) => {
            tracing::debug!(
                "classify_folder_content: {label} path={} read_dir_err={}",
                dir.display(),
                e
            );
            false
        }
    }
}

fn has_root_jar(path: &Path) -> bool {
    dir_has_extension(path, "jar", "root_jar_check")
}

fn has_root_json(path: &Path) -> bool {
    dir_has_extension(path, "json", "root_json_check")
}

/// Whether `mods/` contains at least one `.jar` file.
fn has_mods_jar(path: &Path) -> bool {
    dir_has_extension(&path.join("mods"), "jar", "mods_jar_check")
}

// ─── HMCL data directory discovery ─────────────────────────────────────────

/// Result of looking up a mod file hash on Modrinth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModrinthLookupResult {
    pub hash: String,
    pub project_id: String,
    pub version_id: String,
    pub project_name: Option<String>,
    pub project_slug: Option<String>,
    pub version_number: Option<String>,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
}

/// Look up a mod file by SHA1 hash to find matching Modrinth project and version.
///
/// Computes the SHA1 hash of the given file and queries the Modrinth API
/// to find matching versions. Returns project and version information if found.
pub async fn lookup_mod_hash(
    path: &Path,
) -> crate::Result<Option<ModrinthLookupResult>> {
    let (_, hash) = crate::util::fetch::sha1_file_async(path).await?;

    let state = crate::State::get().await?;

    let files = crate::state::CachedEntry::get_file_many(
        &[&hash],
        Some(crate::state::CacheBehaviour::StaleWhileRevalidateSkipOffline),
        &state.pool,
        &state.api_semaphore,
    )
    .await?;

    if files.is_empty() {
        return Ok(None);
    }

    let file = &files[0];
    let version = crate::state::CachedEntry::get_version(
        &ModrinthVersionId::new(file.version_id.clone())?,
        Some(crate::state::CacheBehaviour::StaleWhileRevalidateSkipOffline),
        &state.pool,
        &state.api_semaphore,
    )
    .await?;

    let project = if let Some(v) = &version {
        crate::state::CachedEntry::get_project(
            &ModrinthProjectId::new(v.project_id.clone())?,
            Some(crate::state::CacheBehaviour::StaleWhileRevalidateSkipOffline),
            &state.pool,
            &state.api_semaphore,
        )
        .await?
    } else {
        None
    };

    Ok(Some(ModrinthLookupResult {
        hash,
        project_id: file.project_id.clone(),
        version_id: file.version_id.clone(),
        project_name: project.as_ref().map(|p| p.title.clone()),
        project_slug: project.as_ref().and_then(|p| p.slug.clone()),
        version_number: version.as_ref().map(|v| v.version_number.clone()),
        game_versions: version
            .as_ref()
            .map(|v| v.game_versions.clone())
            .unwrap_or_default(),
        loaders: version
            .as_ref()
            .map(|v| v.loaders.clone())
            .unwrap_or_default(),
    }))
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
