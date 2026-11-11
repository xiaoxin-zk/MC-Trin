//! Installer for plain zipped-up game folders.
//!
//! These archives have no pack manifest at all — they are a `.minecraft`
//! folder (optionally wrapped in extra directories) identified by a
//! `versions/<id>/<id>.json` structure. The version JSON is inspected to
//! guess the game version and loader, and the folder contents become the
//! instance's game directory.

use std::path::PathBuf;

use serde::Deserialize;

use super::archive_util;
use crate::data::ModLoader;
use crate::install::{
    InstallPhaseDetails, InstallPhaseId, InstallProgressReporter,
};
use crate::state::{
    AppliedContentSetPatch, ContentSourceKind, EditInstance,
    InstanceInstallStage, InstanceLink,
};

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
struct PlainVersionJson {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    inherits_from: Option<String>,
    #[serde(default)]
    libraries: Vec<PlainLibrary>,
}

#[derive(Deserialize, Debug)]
struct PlainLibrary {
    #[serde(default)]
    name: Option<String>,
}

fn looks_like_game_version(value: &str) -> bool {
    let mut parts = value.split('.');
    parts.next().is_some_and(|part| part.parse::<u32>().is_ok())
        && value.split('.').skip(1).all(|part| {
            part.split('-')
                .next()
                .is_some_and(|part| part.parse::<u32>().is_ok())
        })
}

struct DetectedTarget {
    game_version: String,
    loader: ModLoader,
    loader_version: Option<String>,
}

fn detect_target(version_json: &PlainVersionJson) -> Option<DetectedTarget> {
    let mut game_version = version_json
        .inherits_from
        .clone()
        .filter(|value| looks_like_game_version(value));
    let mut loader = ModLoader::Vanilla;
    let mut loader_version = None;

    for library in &version_json.libraries {
        let Some(name) = &library.name else {
            continue;
        };
        let parts: Vec<&str> = name.split(':').collect();
        if parts.len() < 3 {
            continue;
        }
        let (group, artifact, version) = (parts[0], parts[1], parts[2]);
        match (group, artifact) {
            ("net.fabricmc", "fabric-loader") => {
                loader = ModLoader::Fabric;
                loader_version = Some(version.to_string());
            }
            ("org.quiltmc", "quilt-loader") => {
                loader = ModLoader::Quilt;
                loader_version = Some(version.to_string());
            }
            ("net.neoforged", "neoforge" | "forge") => {
                loader = ModLoader::NeoForge;
                loader_version = Some(version.to_string());
            }
            ("net.minecraftforge", "forge" | "fmlloader") => {
                loader = ModLoader::Forge;
                // Forge versions are usually stored as `<mc>-<forge>`.
                let forge_version = version
                    .split_once('-')
                    .map(|(mc, forge)| {
                        if game_version.is_none() && looks_like_game_version(mc)
                        {
                            game_version = Some(mc.to_string());
                        }
                        forge.to_string()
                    })
                    .unwrap_or_else(|| version.to_string());
                loader_version = Some(forge_version);
            }
            ("optifine", "OptiFine") if loader == ModLoader::Vanilla => {
                loader = ModLoader::OptiFine;
                // OptiFine library versions look like `<mc>_HD_U_I6`.
                loader_version = Some(
                    version
                        .split_once('_')
                        .map(|(_, of)| of.to_string())
                        .unwrap_or_else(|| version.to_string()),
                );
            }
            _ => {}
        }
    }

    if game_version.is_none()
        && let Some(id) = &version_json.id
        && looks_like_game_version(id)
    {
        game_version = Some(id.clone());
    }

    game_version.map(|game_version| DetectedTarget {
        game_version,
        loader,
        loader_version,
    })
}

/// Reads every `versions/<id>/<id>.json` under the base folder. Archives of
/// modded installs usually contain both the vanilla and the modded version
/// folder, so all candidates are needed to pick the right one.
///
/// Runs on the blocking pool under the shared detection deadline, because this
/// is the last unbounded read before the caller publishes any progress: opening
/// the central directory, walking every entry and inflating each candidate
/// manifest are all uninterruptible from the async side. Without a deadline and
/// a cooperative token a malformed or slow archive keeps the "parsing" spinner
/// up forever, and a cancelled job leaves this worker running detached.
async fn read_version_candidates(
    archive_path: PathBuf,
    base_folder: String,
    cancellation: tokio_util::sync::CancellationToken,
) -> crate::Result<Vec<(String, PlainVersionJson)>> {
    use super::archive_util::check_cancellation;
    use super::detect::DETECT_TIMEOUT;

    check_cancellation(Some(&cancellation))?;

    let worker_cancellation = cancellation.child_token();
    let mut guard = PlainScanGuard::new(worker_cancellation.clone());
    let worker = tokio::task::spawn_blocking(move || {
        // Inside the worker so the deadline also covers the metadata stat,
        // which blocks on network-backed paths.
        super::detect::check_archive_size(&archive_path)?;
        let file = std::fs::File::open(&archive_path).map_err(|error| {
            crate::util::io::IOError::with_path(error, &archive_path)
        })?;
        let mut archive = zip::ZipArchive::new(file).map_err(|error| {
            crate::ErrorKind::InputError(format!(
                "Modpack archive is invalid: {error}"
            ))
        })?;
        // Earliest point after the constructor where the flag is observable.
        check_cancellation(Some(&worker_cancellation))?;
        let total_entries = archive.len();
        if total_entries > super::detect::MAX_DETECT_ENTRIES {
            return Err(crate::ErrorKind::InputError(format!(
                "Modpack archive has {total_entries} entries, which exceeds the {}-entry detection limit",
                super::detect::MAX_DETECT_ENTRIES
            ))
            .into());
        }
        let versions_prefix = format!("{base_folder}versions/");
        let mut candidates = Vec::new();
        for index in 0..archive.len() {
            check_cancellation(Some(&worker_cancellation))?;
            let name = {
                let entry = archive.by_index_raw(index).map_err(|error| {
                    crate::ErrorKind::InputError(format!(
                        "Failed to read modpack archive entry: {error}"
                    ))
                })?;
                crate::pack::detect::decode_zip_entry_name(entry.name_raw())
            };
            let Some(rest) = name.strip_prefix(&versions_prefix) else {
                continue;
            };
            let mut segments = rest.split('/');
            let (Some(id), Some(json), None) =
                (segments.next(), segments.next(), segments.next())
            else {
                continue;
            };
            if json.strip_suffix(".json") != Some(id) {
                continue;
            }
            let id = id.to_string();
            let mut entry = archive.by_index(index).map_err(|error| {
                crate::ErrorKind::InputError(format!(
                    "Failed to read modpack archive entry: {error}"
                ))
            })?;
            let mut contents = Vec::new();
            // Bound the read itself: a member that declares a huge
            // uncompressed size must not be materialised before any check runs.
            std::io::Read::read_to_end(
                &mut std::io::Read::take(&mut entry, MANIFEST_READ_LIMIT + 1),
                &mut contents,
            )?;
            if contents.len() as u64 > MANIFEST_READ_LIMIT {
                return Err(crate::ErrorKind::InputError(format!(
                    "Modpack version manifest {name} is larger than the {MANIFEST_READ_LIMIT}-byte limit"
                ))
                .into());
            }
            // The read above is a single uninterruptible system call, so a
            // cancellation that landed during it is only visible here.
            check_cancellation(Some(&worker_cancellation))?;
            if let Ok(parsed) =
                serde_json::from_slice::<PlainVersionJson>(&contents)
            {
                candidates.push((id, parsed));
            }
        }
        Ok(candidates)
    });

    match tokio::time::timeout(DETECT_TIMEOUT, worker).await {
        Ok(result) => {
            guard.disarm();
            result?
        }
        Err(_) => {
            guard.cancel();
            Err(crate::ErrorKind::InputError(format!(
                "Timed out after {} seconds while reading the archived instance version manifests",
                DETECT_TIMEOUT.as_secs()
            ))
            .into())
        }
    }
}

/// Largest `versions/<id>/<id>.json` this installer will parse.
///
/// Real version JSONs are tens of KiB; the biggest modded ones stay well under a
/// megabyte. The bound exists so a hostile or corrupt member cannot be inflated
/// into memory during the parse phase.
const MANIFEST_READ_LIMIT: u64 = 4 * 1024 * 1024;

/// Cancels a version-candidate scan when its awaiting future is dropped.
///
/// A dropped `spawn_blocking` join handle does not stop the task, so without
/// this the worker would keep walking the archive after the job was cancelled.
struct PlainScanGuard {
    token: tokio_util::sync::CancellationToken,
    active: bool,
}

impl PlainScanGuard {
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

impl Drop for PlainScanGuard {
    fn drop(&mut self) {
        if self.active {
            self.token.cancel();
        }
    }
}

pub(crate) async fn install_plain_archive_with_reporter(
    instance_id: String,
    archive_path: PathBuf,
    base_folder: String,
    version_id: String,
    source_filename: Option<String>,
    reporter: InstallProgressReporter,
) -> crate::Result<()> {
    // Pull the job's cancellation token before the blocking read, so a cancel
    // that arrives during the scan reaches the worker instead of leaving it
    // detached. Errors below (timeout, cancel) propagate as terminal job
    // failures rather than falling back to a guessed target.
    let candidates = read_version_candidates(
        archive_path.clone(),
        base_folder.clone(),
        reporter.cancellation_token(),
    )
    .await?;
    let mut targets: Vec<(String, DetectedTarget)> = candidates
        .iter()
        .filter_map(|(id, json)| {
            detect_target(json).map(|target| (id.clone(), target))
        })
        .collect();
    let selected = targets
        .iter()
        .position(|(_, target)| target.loader != ModLoader::Vanilla)
        .map(|index| targets.remove(index))
        .or_else(|| {
            if targets.is_empty() {
                None
            } else {
                Some(targets.remove(0))
            }
        });
    let Some((selected_id, target)) = selected else {
        return Err(crate::ErrorKind::InputError(format!(
            "Could not determine the Minecraft version of archived instance {version_id}"
        ))
        .into());
    };

    let pack_name = if selected_id.trim().is_empty() {
        source_filename
            .as_ref()
            .map(|name| {
                std::path::Path::new(name)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_else(|| "Imported Instance".to_string())
    } else {
        selected_id.clone()
    };
    let pack_details = InstallPhaseDetails::Modpack {
        project_id: None,
        version_id: None,
        title: Some(pack_name.clone()),
    };
    reporter
        .update(InstallPhaseId::ResolvingPack, None, pack_details.clone())
        .await?;

    let resolved_loader_version = if target.loader != ModLoader::Vanilla {
        crate::launcher::get_loader_version_from_profile(
            &target.game_version,
            target.loader,
            target.loader_version.as_deref(),
        )
        .await?
    } else {
        None
    };

    crate::api::instance::edit(
        &instance_id,
        EditInstance {
            install_stage: Some(InstanceInstallStage::PackInstalling),
            name: Some(pack_name.clone()),
            link: Some(InstanceLink::ImportedModpack {
                project_id: None,
                version_id: None,
                name: Some(pack_name),
                version_number: None,
                filename: source_filename,
            }),
            content_set_patch: Some(AppliedContentSetPatch {
                source_kind: Some(ContentSourceKind::ImportedModpack),
                game_version: Some(target.game_version.clone()),
                protocol_version: Some(None),
                loader: Some(target.loader),
                loader_version: Some(
                    resolved_loader_version.map(|version| version.id),
                ),
            }),
            ..EditInstance::default()
        },
    )
    .await?;

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
            base_folder,
            instance_path.clone(),
        )
        .await?;

    let post_archive_result: crate::Result<()> = async {
        let local_source =
            crate::launcher::download::LocalRuntimeSource::discover(
                &instance_path,
            );
        crate::launcher::install_minecraft_for_instance_id_with_local_source(
            &instance_id,
            local_source,
            false,
            Some(reporter.clone()),
            crate::launcher::InstanceCompletionPolicy::DeferToInstallJob,
        )
        .await?;
        reporter.clear_context().await?;
        Ok(())
    }
    .await;
    archive_util::settle_staged_archive_install(
        instance_id,
        archive_cancellation,
        archive_replacements,
        post_archive_result,
        "plain ZIP contents",
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn write_pack(entries: &[(&str, &[u8])]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.zip");
        std::fs::write(&path, build_archive(entries)).unwrap();
        (dir, path)
    }

    #[tokio::test]
    async fn reads_every_version_candidate_under_the_base() {
        let (_dir, path) = write_pack(&[
            (
                "pack/versions/1.20.1/1.20.1.json",
                br#"{"id":"1.20.1"}"# as &[u8],
            ),
            (
                "pack/versions/forge-1.20.1/forge-1.20.1.json",
                br#"{"id":"forge-1.20.1","inheritsFrom":"1.20.1"}"# as &[u8],
            ),
        ]);

        let candidates = read_version_candidates(
            path,
            "pack/".to_string(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|(id, _)| id == "1.20.1"));
        assert!(candidates.iter().any(|(id, _)| id == "forge-1.20.1"));
    }

    #[tokio::test]
    async fn a_cancelled_token_fails_before_reading_the_archive() {
        // The pre-read check must turn a cancelled job into a terminal error
        // instead of scanning the archive anyway.
        let (_dir, path) = write_pack(&[(
            "pack/versions/1.20.1/1.20.1.json",
            br#"{"id":"1.20.1"}"# as &[u8],
        )]);
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();

        let error =
            read_version_candidates(path, "pack/".to_string(), cancellation)
                .await
                .unwrap_err();

        assert!(
            error.to_string().contains("canceled"),
            "expected the cancellation error, got: {error}"
        );
    }

    #[tokio::test]
    async fn a_manifest_over_the_reader_limit_is_rejected() {
        // The version JSON read is bounded, so an oversized member produces a
        // clear error rather than being inflated into memory.
        let oversized = vec![b' '; (MANIFEST_READ_LIMIT + 1) as usize];
        let (_dir, path) =
            write_pack(&[("pack/versions/1.20.1/1.20.1.json", &oversized)]);

        let error = read_version_candidates(
            path,
            "pack/".to_string(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap_err();

        assert!(
            error.to_string().contains("larger than the"),
            "expected the size-limit error, got: {error}"
        );
    }
}
