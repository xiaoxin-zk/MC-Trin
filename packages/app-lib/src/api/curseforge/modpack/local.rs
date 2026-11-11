//! CurseForge modpack install local.

use super::*;
use crate::api::curseforge::archive::{
    game_options_pack_source_at, loader_type, materialize_modpack_overrides_at,
    modpack_servers_at, modpack_target, modpack_zip_error,
    read_modpack_manifest_at, settle_materialized_modpack_overrides,
};
use crate::api::curseforge::catalog::{
    get_modpack_files, get_modpack_projects,
};
use crate::api::curseforge::download::resolve_download_url as resolve_curseforge_download_url;
use crate::api::curseforge::dto::{
    CurseForgeFile, CurseForgeInstallRequest, CurseForgeInstallResult,
    CurseForgeInstalledFile, CurseForgeManifestFile,
    CurseForgeModpackInstallResult, CurseForgeProject,
};
use crate::api::curseforge::metrics::CurseForgeDownloadMetrics;
use crate::api::curseforge::selection::{
    content_target_folder, managed_project_type, manual_download_from_file,
    persist_manual_download,
};
use crate::api::curseforge::validation::validate_file_name;
use crate::api::curseforge::verify::{
    DownloadInstalledFileRequest, download_installed_file,
};
use crate::install::{
    InstallJobEventKind, InstallPhaseDetails, InstallPhaseId, InstallProgress,
    InstallProgressReporter, InstallProgressSecondary,
};
use crate::state::{
    ContentSourceKind, EditInstance, InstanceLink, ModLoader, Settings,
};
use crate::{ErrorKind, State};
use std::collections::HashSet;
use std::sync::Arc;

pub(super) async fn install_preloaded_modpack_file(
    request: &CurseForgeInstallRequest,
    project: &CurseForgeProject,
    file: &CurseForgeFile,
    download_metrics: Option<&CurseForgeDownloadMetrics>,
) -> crate::Result<CurseForgeInstallResult> {
    if project.id != request.project_id
        || file.mod_id != request.project_id
        || file.id != request.file_id
    {
        return Err(ErrorKind::InputError(
            "Preloaded CurseForge metadata does not match the manifest file"
                .to_string(),
        )
        .into());
    }
    let project_type = managed_project_type(&request.project_type)?;
    let download_url = resolve_curseforge_download_url(
        request.project_id,
        request.file_id,
        project,
        file,
    )
    .await?;
    let Some(download_url) = download_url else {
        let target_folder =
            content_target_folder(project_type, request.world_name.as_deref())?;
        let manual_download = manual_download_from_file(
            request.project_id,
            request.file_id,
            file,
            project,
            project_type.get_name(),
            target_folder,
            request.ownership_kind,
            request.manual_operation_kind,
        );
        persist_manual_download(&request.instance_id, &manual_download).await?;
        return Ok(CurseForgeInstallResult {
            manual_downloads: vec![manual_download],
            ..Default::default()
        });
    };
    validate_file_name(&file.file_name)?;
    let downloaded = download_installed_file(DownloadInstalledFileRequest {
        instance_id: &request.instance_id,
        url: &download_url,
        file,
        project_type,
        world_name: request.world_name.as_deref(),
        project_id: request.project_id,
        file_id: request.file_id,
        project_slug: &project.slug,
        ownership_kind: request.ownership_kind,
        download_metrics,
        defer_persistence: request.defer_persistence,
        verification_tx: request.verification_tx.as_ref(),
        pre_resolved_relative_path: request
            .pre_resolved_relative_path
            .as_deref(),
        expected_file_name: request.expected_file_name.as_deref(),
    })
    .await?;
    Ok(CurseForgeInstallResult {
        installed: vec![CurseForgeInstalledFile {
            project_id: request.project_id,
            file_id: request.file_id,
            relative_path: downloaded.relative_path,
            dependency: false,
        }],
        ..Default::default()
    })
}

pub(super) fn install_error_chain(error: &crate::Error) -> String {
    let mut chain = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        chain.push_str("\nCaused by: ");
        chain.push_str(&cause.to_string());
        source = cause.source();
    }
    chain
}

/// Installs a CurseForge modpack from a local archive on disk (a zip with a
/// `manifest.json`), downloading the listed files through the CurseForge API
/// and extracting the overrides folder. Unlike [`install_modpack_with_reporter`]
/// this needs no project/file id — undownloadable files land on the manual
/// download list exactly like API-driven installs.
/// Cancels the initial metadata parse when its awaiting future is dropped.
///
/// A dropped `spawn_blocking` join handle does not stop the task, so a job
/// cancelled while this worker runs would otherwise keep walking the archive
/// with nobody able to observe the result.
struct LocalArchiveParseGuard {
    token: tokio_util::sync::CancellationToken,
    active: bool,
}

impl LocalArchiveParseGuard {
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

impl Drop for LocalArchiveParseGuard {
    fn drop(&mut self) {
        if self.active {
            self.token.cancel();
        }
    }
}

pub async fn install_modpack_from_local_archive_with_reporter(
    instance_id: String,
    archive_path: std::path::PathBuf,
    base_folder: String,
    source_filename: Option<String>,
    install_optional: bool,
    reporter: InstallProgressReporter,
    completion_policy: crate::launcher::InstanceCompletionPolicy,
) -> crate::Result<CurseForgeModpackInstallResult> {
    // Initial metadata parse. This runs before the caller publishes any
    // progress, so an unbounded worker here is indistinguishable from a hang.
    // It gets the shared detection deadline and the job's cancellation token:
    // opening the central directory, reading the manifest and walking every
    // entry for options/servers are all uninterruptible from the async side.
    let manifest_archive_path = archive_path.clone();
    let manifest_base_folder = base_folder.clone();
    let cancellation = reporter.cancellation_token();
    let mut parse_guard =
        LocalArchiveParseGuard::new(cancellation.child_token());
    let worker_cancellation = parse_guard.token.clone();
    let worker = tokio::task::spawn_blocking(move || {
        // Inside the worker so the deadline also covers the metadata stat,
        // which blocks on network-backed paths.
        crate::api::pack::detect::check_archive_size(&manifest_archive_path)?;
        let file = std::fs::File::open(&manifest_archive_path)?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(modpack_zip_error)?;
        // Earliest point after the constructor where the flag is observable.
        crate::api::pack::archive_util::check_cancellation(Some(
            &worker_cancellation,
        ))?;
        // Refuse a pathological entry count before walking the archive for
        // options/servers: without this a huge archive could burn the whole
        // deadline inside those loops instead of failing fast.
        let total_entries = archive.len();
        if total_entries > crate::api::pack::detect::MAX_DETECT_ENTRIES {
            return Err(crate::ErrorKind::InputError(format!(
                "CurseForge modpack archive has {total_entries} entries, which exceeds the {}-entry detection limit",
                crate::api::pack::detect::MAX_DETECT_ENTRIES
            ))
            .into());
        }
        let manifest = read_modpack_manifest_at(
            &mut archive,
            &manifest_base_folder,
            Some(&worker_cancellation),
        )?;
        let game_options_source = game_options_pack_source_at(
            &mut archive,
            &manifest,
            &manifest_base_folder,
            Some(&worker_cancellation),
        )?;
        let pack_servers = modpack_servers_at(
            &mut archive,
            &manifest,
            &manifest_base_folder,
            Some(&worker_cancellation),
        )?;
        Ok::<_, crate::Error>((manifest, game_options_source, pack_servers))
    });

    let (manifest, game_options_source, pack_servers) =
        match tokio::time::timeout(
            crate::api::pack::detect::DETECT_TIMEOUT,
            worker,
        )
        .await
        {
            Ok(result) => {
                parse_guard.disarm();
                result??
            }
            Err(_) => {
                parse_guard.cancel();
                return Err(crate::ErrorKind::InputError(format!(
                    "Timed out after {} seconds while reading the CurseForge modpack archive",
                    crate::api::pack::detect::DETECT_TIMEOUT.as_secs()
                ))
                .into());
            }
        };

    let target = modpack_target(&manifest)?;
    let loader = (target.loader != ModLoader::Vanilla)
        .then(|| target.loader.as_str().to_string());
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
        .unwrap_or_else(|| "CurseForge Modpack".to_string());
    let pack_details = InstallPhaseDetails::Modpack {
        project_id: None,
        version_id: None,
        title: Some(pack_name.clone()),
    };
    reporter
        .update(InstallPhaseId::ResolvingPack, None, pack_details.clone())
        .await?;

    let loader_version = if target.loader != ModLoader::Vanilla {
        crate::launcher::get_loader_version_from_profile(
            &manifest.minecraft.version,
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
            install_stage: Some(
                crate::state::InstanceInstallStage::PackInstalling,
            ),
            name: Some(pack_name.clone()),
            link: Some(InstanceLink::ImportedModpack {
                project_id: None,
                version_id: None,
                name: Some(pack_name.clone()),
                version_number: manifest.version.clone(),
                filename: source_filename,
            }),
            content_set_patch: Some(crate::state::AppliedContentSetPatch {
                source_kind: Some(ContentSourceKind::CurseForge),
                game_version: Some(manifest.minecraft.version.clone()),
                protocol_version: Some(None),
                loader: Some(target.loader),
                loader_version: Some(loader_version.map(|version| version.id)),
            }),
            ..EditInstance::default()
        },
    )
    .await?;

    let minecraft_install = (completion_policy
        == crate::launcher::InstanceCompletionPolicy::DeferToInstallJob)
        .then(|| {
            crate::api::pack::parallel_minecraft_install::ParallelMinecraftInstall::start(
                instance_id.clone(),
                reporter.clone(),
            )
        });

    let content = install_local_manifest_files(
        &instance_id,
        manifest.files.clone(),
        install_optional,
        &manifest.minecraft.version,
        loader.as_deref(),
        pack_details.clone(),
        &reporter,
    )
    .await?;

    if !content.manual_downloads.is_empty() {
        if let Some(minecraft_install) = minecraft_install {
            minecraft_install.abort().await;
        }
        return Ok(CurseForgeModpackInstallResult {
            content,
            overrides_written: 0,
            minecraft_version: manifest.minecraft.version,
            loader,
            game_options_source,
            pack_servers,
        });
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
    let overrides_archive_path = archive_path.clone();
    // A wrapped pack keeps its override tree below the detected prefix, so the
    // same `base_folder` used for the manifest has to reach the extractor too.
    let overrides_base_folder = base_folder.clone();
    let override_cancellation = reporter.cancellation_token();
    let (overrides_written, override_replacements) =
        crate::api::pack::archive_util::run_cancellable_blocking_instance_write(
            instance_id.clone(),
            override_cancellation.clone(),
            move |cancellation| {
                materialize_modpack_overrides_at(
                    &overrides_archive_path,
                    &instance_path,
                    &overrides_base_folder,
                    Some(cancellation),
                )
            },
        )
        .await?;

    let post_override_result: crate::Result<()> = async {
        if let Some(minecraft_install) = minecraft_install {
            minecraft_install.join().await?;
        } else {
            crate::launcher::install_minecraft_for_instance_id_with_reporter(
                &instance_id,
                false,
                Some(reporter.clone()),
                completion_policy,
            )
            .await?;
        }
        reporter.clear_context().await?;
        Ok(())
    }
    .await;
    match post_override_result {
        Ok(()) => {
            settle_materialized_modpack_overrides(
                instance_id.clone(),
                override_cancellation,
                override_replacements,
                true,
            )
            .await?;
        }
        Err(error) => {
            if let Err(rollback_error) = settle_materialized_modpack_overrides(
                instance_id.clone(),
                override_cancellation,
                override_replacements,
                false,
            )
            .await
            {
                return Err(ErrorKind::OtherError(format!(
                    "{error}; failed to restore CurseForge ZIP overrides: {rollback_error}"
                ))
                .into());
            }
            return Err(error);
        }
    }

    Ok(CurseForgeModpackInstallResult {
        content,
        overrides_written,
        minecraft_version: manifest.minecraft.version,
        loader,
        game_options_source,
        pack_servers,
    })
}

/// Downloads and installs the files listed in a local CurseForge manifest.
/// Mirrors the file loop of [`install_modpack_with_reporter`], simplified to
/// the job-reporter path used by local imports. Also used by the MCBBS
/// installer for its CurseForge-style `files` array.
pub(crate) async fn install_local_manifest_files(
    instance_id: &str,
    manifest_files: Vec<CurseForgeManifestFile>,
    install_optional: bool,
    minecraft_version: &str,
    loader: Option<&str>,
    pack_details: InstallPhaseDetails,
    reporter: &InstallProgressReporter,
) -> crate::Result<CurseForgeInstallResult> {
    let reporter = &reporter.clone().with_pipeline_cancellation();
    let state = State::get().await?;
    let content_set = {
        let mut connection = state.pool.acquire().await?;
        crate::state::instances::adapters::sqlite::content_rows::get_applied_content_set(
            instance_id,
            &mut *connection,
        )
        .await?
    }
    .ok_or_else(|| {
        ErrorKind::InputError(
            "Instance has no applied content set".to_string(),
        )
    })?;
    let installed_releases = crate::state::instances::adapters::sqlite::content_rows::get_pack_members(
        &content_set.id,
        &state.pool,
    )
    .await?
    .into_iter()
    .filter(|member| {
        member.materialization_state
            == crate::state::instances::PackMemberMaterializationState::Present
    })
    .filter_map(|member| {
        Some((member.provider_project_id?, member.provider_release_id?))
    })
    .collect::<HashSet<_>>();
    let selected_files = manifest_files
        .into_iter()
        .filter(|file| file.required || install_optional)
        .filter(|file| {
            !installed_releases.contains(&(
                file.project_id.to_string(),
                file.file_id.to_string(),
            ))
        })
        .collect::<Vec<_>>();
    let loader_type_value = loader.and_then(loader_type);
    let project_ids = selected_files
        .iter()
        .map(|file| file.project_id)
        .collect::<Vec<_>>();
    let projects = get_modpack_projects(project_ids).await?;

    let total_files = selected_files.len().max(1);
    let file_ids = selected_files
        .iter()
        .map(|file| file.file_id)
        .collect::<Vec<_>>();
    let file_meta = get_modpack_files(file_ids).await?;
    let existing_relative_paths = Arc::new(
        {
            let mut connection = state.pool.acquire().await?;
            crate::state::instances::adapters::sqlite::content_rows::get_instance_files(
                instance_id,
                &mut *connection,
            )
            .await?
        }
        .into_iter()
        .map(|file| file.relative_path)
        .collect::<HashSet<_>>(),
    );
    let prefer_localized_names = {
        let mut connection = state.pool.acquire().await?;
        Settings::get_with_connection(&mut connection)
            .await?
            .resource_chinese_filenames
    };
    let content_total_bytes = selected_files
        .iter()
        .map(|file| {
            file_meta
                .get(&file.file_id)
                .map(|meta| meta.file_length)
                .unwrap_or(0)
        })
        .sum::<u64>();
    reporter
        .update_with_events(
            InstallPhaseId::DownloadingContent,
            Some(InstallProgress {
                current: 0,
                total: total_files as u64,
                secondary: Some(InstallProgressSecondary {
                    current: 0,
                    total: content_total_bytes,
                }),
            }),
            pack_details.clone(),
            vec![InstallJobEventKind::ContentDownloadStarted {
                files: total_files as u64,
                bytes: Some(content_total_bytes),
            }],
        )
        .await?;

    install_pack_files(
        PackFileInstall {
            source: "local",
            instance_id,
            minecraft_version,
            loader_type_value,
            operation_kind:
                crate::state::instances::ManualDownloadOperationKind::PackUpdate,
            pack_details,
            reporter: Some(reporter.clone()),
            loading_bar: None,
            download_metrics: Some(Arc::new(
                CurseForgeDownloadMetrics::with_reporter(reporter.clone()),
            )),
            selected_files,
            projects,
            file_meta,
            existing_relative_paths,
            prefer_localized_names,
            total_files,
            content_total_bytes,
        },
        &state,
    )
    .await
}
