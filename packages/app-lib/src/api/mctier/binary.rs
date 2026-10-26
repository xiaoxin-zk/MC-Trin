use eyre::{Context, bail};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read},
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::PreparationStage;

const VERSION: &str = "2.6.4";
const MAX_DOWNLOAD: usize = 128 * 1024 * 1024;

// Mirrors published on https://easytier.cn/guide/download.html, ordered by the
// throughput measured without an HTTP proxy. The origin stays as the fallback.
const DOWNLOAD_BASES: [&str; 5] = [
    "https://v6.gh-proxy.org/",
    "https://cdn.gh-proxy.org/",
    "https://hk.gh-proxy.org/",
    "https://edgeone.gh-proxy.org/",
    "https://github.com/",
];

const PER_SOURCE_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(120);
// Permit all five direct sources and all five proxy-backed sources to reach
// their per-source timeout, with time left for extraction and verification.
const DOWNLOAD_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(1500);

pub fn release_for(
    os: &str,
    arch: &str,
) -> eyre::Result<(&'static str, &'static str)> {
    Ok(match (os, arch) {
        ("windows", "x86_64") => (
            "windows-x86_64",
            "27af91e270e554709b048bd32327fefd2dfce5062ae1e8701af7550c6f525f84",
        ),
        ("windows", "aarch64") => (
            "windows-arm64",
            "37023f8a3451c9234b17ee2089a03dc344ce90d803b5b359cb6c46682b0549b4",
        ),
        ("linux", "x86_64") => (
            "linux-x86_64",
            "61b659eaedba658fa66fe47d17e1426cdd77e5d02fa15fed447bb4357c09dfd6",
        ),
        ("linux", "aarch64") => (
            "linux-aarch64",
            "f533ec25a7ea714e09f645615012200278058525795cc3bb690ff011aec1a70f",
        ),
        ("macos", "x86_64") => (
            "macos-x86_64",
            "89fc28a6e6995259d76ce3f11775220e8a21c760e94df91a6a9db30a69b6982e",
        ),
        ("macos", "aarch64") => (
            "macos-aarch64",
            "4be1882d1aa36d31c1d6ba0596f2cf8a097e371f8da124212324b2e0f8df7e4b",
        ),
        _ => bail!("unsupported EasyTier platform"),
    })
}

pub fn hidden_command(path: &Path) -> Command {
    let mut command = Command::new(path);
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    // An existing EasyTier environment must not enable unrelated services.
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("ET_") {
            command.env_remove(name);
        }
    }
    command
}

/// Percentage of the archive received so far. The last unit is reserved for the
/// SHA-256 check so the bar never claims completion before it is verified.
fn download_percent(downloaded: usize, total: u64) -> u8 {
    let share = downloaded as u64 * 100 / total.max(1);
    share.min(99) as u8
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn extract(bytes: Vec<u8>, root: &Path) -> eyre::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    if std::fs::symlink_metadata(root)?.file_type().is_symlink() {
        bail!("EasyTier runtime path must not be a link");
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let core_name = if cfg!(windows) {
        "easytier-core.exe"
    } else {
        "easytier-core"
    };
    let mut core = None;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(relative) = entry.enclosed_name() else {
            bail!("invalid archive path");
        };
        let Some(name) = relative.file_name().and_then(|name| name.to_str())
        else {
            continue;
        };
        // No service installers or driver files are extracted or executed.
        let needed = name == core_name
            || (cfg!(windows) && name.ends_with(".dll"))
            || name.starts_with("LICENSE")
            || name.starts_with("COPYING");
        if !needed || entry.is_dir() {
            continue;
        }
        if entry.size() > 192 * 1024 * 1024 {
            bail!("EasyTier resource is too large");
        }
        let mut contents = Vec::new();
        entry.read_to_end(&mut contents)?;
        let target = root.join(name);
        if let Ok(metadata) = std::fs::symlink_metadata(&target)
            && (metadata.file_type().is_symlink() || !metadata.is_file())
        {
            bail!("EasyTier resource path must be a regular file");
        }
        if std::fs::read(&target).ok().as_deref() != Some(contents.as_slice()) {
            let temporary = root.join(format!(".{name}.download"));
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            use std::io::Write;
            file.write_all(&contents)?;
            file.sync_all()?;
            drop(file);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    &temporary,
                    std::fs::Permissions::from_mode(0o700),
                )?;
            }
            if target.exists() {
                std::fs::remove_file(&target)?;
            }
            std::fs::rename(&temporary, &target)?;
        }
        if name == core_name {
            core = Some(target);
        }
    }
    core.ok_or_else(|| {
        eyre::eyre!("official EasyTier archive has no core executable")
    })
}

pub async fn prepare(
    cancellation: &CancellationToken,
    progress: &(dyn Fn(PreparationStage, Option<u8>) + Sync),
) -> eyre::Result<PathBuf> {
    let (platform, expected) =
        release_for(std::env::consts::OS, std::env::consts::ARCH)?;
    let base = crate::state::DirectoryInfo::global_handle_if_ready()
        .map(|directories| directories.config_dir.clone())
        .or_else(|| {
            crate::state::DirectoryInfo::initial_settings_dir_path(
                crate::brand::BUNDLE_IDENTIFIER,
            )
        })
        .ok_or_else(|| eyre::eyre!("launcher data directory is unavailable"))?;
    let root = base.join("mctier").join(VERSION).join(platform);
    let archive_path = root.join("official.zip");
    let work = async {
        let cached = tokio::fs::read(&archive_path)
            .await
            .ok()
            .filter(|bytes| hash(bytes) == expected);
        let bytes = if let Some(cached) = cached {
            cached
        } else {
            let archive = format!(
                "EasyTier/EasyTier/releases/download/v{VERSION}/easytier-{platform}-v{VERSION}.zip"
            );
            // Direct connections come first because a proxy is routinely slower
            // than the mirrors. The proxied client is only tried when the user
            // opted into the proxy for multiplayer networking.
            let direct = crate::util::fetch::file_reqwest_client_builder()
                .no_proxy()
                .build()?;
            let mut modes = vec![false];
            if crate::util::fetch::multiplayer_uses_proxy().await? {
                modes.push(true);
            }
            let mut failures: Vec<String> = Vec::new();
            let mut downloaded: Option<Vec<u8>> = None;
            'sources: for use_proxy in modes {
                let client = if use_proxy {
                    match crate::util::fetch::configured_client().await {
                        Ok(client) => Some(client),
                        Err(error) => {
                            failures
                                .push(format!("proxy client unavailable: {error:#}"));
                            None
                        }
                    }
                } else {
                    Some(direct.clone())
                };
                let Some(client) = client else { continue };
                for base in DOWNLOAD_BASES {
                    if cancellation.is_cancelled() {
                        bail!("lobby operation cancelled");
                    }
                    let url = format!("{base}{archive}");
                    // The size is unknown until a response arrives, so the bar
                    // stays indeterminate while a new source is connecting.
                    progress(PreparationStage::Downloading, None);
                    let attempt = async {
                        let response =
                            client.get(&url).send().await?.error_for_status()?;
                        if response
                            .content_length()
                            .is_some_and(|size| size > MAX_DOWNLOAD as u64)
                        {
                            bail!("EasyTier download exceeds the size limit");
                        }
                        let total = response.content_length();
                        let mut stream = response.bytes_stream();
                        let mut bytes = Vec::new();
                        let mut reported = None;
                        while let Some(chunk) = stream.next().await {
                            let chunk = chunk?;
                            if bytes.len() + chunk.len() > MAX_DOWNLOAD {
                                bail!(
                                    "EasyTier download exceeds the size limit"
                                );
                            }
                            bytes.extend_from_slice(&chunk);
                            if let Some(total) = total {
                                let percent = download_percent(bytes.len(), total);
                                if reported != Some(percent) {
                                    reported = Some(percent);
                                    progress(
                                        PreparationStage::Downloading,
                                        Some(percent),
                                    );
                                }
                            }
                        }
                        if hash(&bytes) != expected {
                            bail!(
                                "official EasyTier archive failed SHA-256 verification"
                            );
                        }
                        Ok::<Vec<u8>, eyre::Report>(bytes)
                    };
                    match tokio::time::timeout(PER_SOURCE_TIMEOUT, attempt)
                        .await
                    {
                        Ok(Ok(bytes)) => {
                            downloaded = Some(bytes);
                            break 'sources;
                        }
                        Ok(Err(error)) => {
                            failures.push(format!("{url}: {error:#}"))
                        }
                        Err(_) => failures.push(format!("{url}: timed out")),
                    }
                }
            }
            let Some(bytes) = downloaded else {
                bail!(
                    "EasyTier download failed from every source: {}",
                    failures.join("; ")
                );
            };
            tokio::fs::create_dir_all(&root).await?;
            tokio::fs::write(&archive_path, &bytes).await?;
            bytes
        };
        progress(PreparationStage::Extracting, None);
        let output_root = root.clone();
        let path =
            tokio::task::spawn_blocking(move || extract(bytes, &output_root))
                .await??;
        progress(PreparationStage::Verifying, None);
        let output = hidden_command(&path)
            .arg("--help")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;
        let help = String::from_utf8_lossy(&output.stdout);
        if !output.status.success()
            || !help.contains("--port-forward")
            || !help.contains("--no-tun")
        {
            bail!("EasyTier does not support no-TUN TCP forwarding");
        }
        Ok(path)
    };
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => bail!("lobby operation cancelled"),
        result = tokio::time::timeout(DOWNLOAD_TIMEOUT, work) => {
            result.wrap_err("EasyTier preparation timed out")?
        }
    }
}
