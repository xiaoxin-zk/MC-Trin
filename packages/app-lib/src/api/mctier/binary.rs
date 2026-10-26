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

const VERSION: &str = "2.6.4";
const MAX_DOWNLOAD: usize = 128 * 1024 * 1024;

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
            let client = crate::util::fetch::configured_client().await?;
            let response = client.get(format!("https://github.com/EasyTier/EasyTier/releases/download/v{VERSION}/easytier-{platform}-v{VERSION}.zip")).send().await?.error_for_status()?;
            if response
                .content_length()
                .is_some_and(|size| size > MAX_DOWNLOAD as u64)
            {
                bail!("EasyTier download exceeds the size limit");
            }
            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                if bytes.len() + chunk.len() > MAX_DOWNLOAD {
                    bail!("EasyTier download exceeds the size limit");
                }
                bytes.extend_from_slice(&chunk);
            }
            if hash(&bytes) != expected {
                bail!("official EasyTier archive failed SHA-256 verification");
            }
            tokio::fs::create_dir_all(&root).await?;
            tokio::fs::write(&archive_path, &bytes).await?;
            bytes
        };
        let output_root = root.clone();
        let path =
            tokio::task::spawn_blocking(move || extract(bytes, &output_root))
                .await??;
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
        result = tokio::time::timeout(std::time::Duration::from_secs(180), work) => result.wrap_err("EasyTier preparation timed out")?,
    }
}
