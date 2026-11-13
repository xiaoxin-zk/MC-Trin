use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

const PORTABLE_DIR_NAME: &str = ".MC-Trin";
/// Folder name used by earlier builds. It is renamed in place on first run so
/// existing portable installations keep their data across the rename.
const LEGACY_PORTABLE_DIR_NAME: &str = ".Axolotl";

static PORTABLE_MODE: AtomicBool = AtomicBool::new(false);

/// Directory holding the portable installation's runtime state, recorded by
/// [`init_portable_mode`] once it has resolved the marker.
static PORTABLE_DIR: std::sync::Mutex<Option<PathBuf>> =
    std::sync::Mutex::new(None);

/// Returns the portable directory when the launcher is running in portable mode.
///
/// The native webview keeps its user data folder - caches, GPU shader cache,
/// cookies - in the app local data directory unless it is told otherwise. For a
/// portable installation that belongs beside the executable, not in
/// `%LOCALAPPDATA%`, or roughly 18 MB of browser state lands outside the single
/// folder the portable package promises.
pub fn webview_data_directory() -> Option<PathBuf> {
    PORTABLE_DIR.lock().ok()?.clone()
}

/// 在启动时初始化便携模式
/// 检查 `.MC-Trin` 文件夹是否存在且可写
/// 如果存在且可写，将 `MCTRIN_CONFIG_DIR` 环境变量设置为该路径
/// 返回 `true` 如果便携模式已启用，否则返回 `false`
///
/// 必须在 main() 开头、任何其他线程（包括 tokio runtime）启动之前调用。
/// 此函数内部会调用 `std::env::set_var`，该函数在 Rust 中不是线程安全的。
pub unsafe fn init_portable_mode() -> bool {
    let Ok(exe_path) = std::env::current_exe() else {
        return false;
    };

    let Some(app_dir) = exe_path.parent() else {
        return false;
    };

    let requested_dir = app_dir.join(PORTABLE_DIR_NAME);
    let portable_dir = if requested_dir.is_dir() {
        requested_dir
    } else {
        let legacy_dir = app_dir.join(LEGACY_PORTABLE_DIR_NAME);
        if !legacy_dir.is_dir() {
            return false;
        }
        match std::fs::rename(&legacy_dir, &requested_dir) {
            Ok(()) => {
                tracing::info!(
                    "Migrated portable directory to {}",
                    requested_dir.display()
                );
                requested_dir
            }
            Err(error) => {
                tracing::warn!(
                    "Could not rename {} ({}), continuing with it",
                    legacy_dir.display(),
                    error
                );
                legacy_dir
            }
        }
    };

    if !portable_dir.is_dir() {
        return false;
    }
    if !try_write_test(&portable_dir) {
        return false;
    }

    // SAFETY: 调用者保证此时没有其他线程访问环境变量
    unsafe {
        std::env::set_var("MCTRIN_CONFIG_DIR", &portable_dir);
    }
    PORTABLE_MODE.store(true, Ordering::Relaxed);
    if let Ok(mut slot) = PORTABLE_DIR.lock() {
        *slot = Some(portable_dir.clone());
    }

    tracing::info!(
        "Portable mode enabled: MCTRIN_CONFIG_DIR={}",
        portable_dir.display()
    );

    true
}

/// 尝试在目标目录中创建并删除临时文件以验证可写性。
/// 先删除可能残留的 `.write_test`，避免已有的只读文件导致 `File::create` 失败。
fn try_write_test(dir: &std::path::Path) -> bool {
    let test_file = dir.join(".write_test");

    let _ = std::fs::remove_file(&test_file);

    match std::fs::File::create(&test_file) {
        Ok(_) => {
            let _ = std::fs::remove_file(&test_file);
            true
        }
        Err(e) => {
            tracing::warn!(
                "Portable directory {} exists but is not writable: {}",
                dir.display(),
                e
            );
            false
        }
    }
}

/// Tauri 命令：检查应用程序是否运行在便携模式下
#[tauri::command]
pub fn is_portable_mode() -> bool {
    PORTABLE_MODE.load(Ordering::Relaxed)
}
