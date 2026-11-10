#[cfg(target_os = "windows")]
mod windows {
    use super::super::{DirectoryInfo, Settings};
    use sqlx::SqlitePool;
    use std::path::{Path, PathBuf};
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};

    /// Keys the installer may have written its choices under, most specific
    /// first. The first entry matches the key the NSIS script derives from the
    /// bundle's manufacturer and product name; the second is the upstream
    /// project's key, kept so installs from older builds still resolve.
    const INSTALLER_REGISTRY_KEYS: [&str; 2] = [
        "Software\\mctrin\\MC-Trin",
        "Software\\ghs\\Axolotl Launcher",
    ];
    const PENDING_RESOURCE_DIRECTORY_VALUE: &str = "PendingResourceDirectory";

    #[derive(Debug, Eq, PartialEq)]
    enum PendingDirectoryDecision {
        Apply {
            custom_dir: String,
            prev_custom_dir: String,
        },
        Clear,
    }

    fn normalize_directory(path: &str) -> Option<String> {
        let path = path.trim();
        if path.is_empty() {
            return None;
        }

        let path = PathBuf::from(path);
        if !path.is_absolute() || path.parent().is_none() {
            return None;
        }

        let mut normalized = path.to_string_lossy().replace('/', "\\");
        while normalized.len() > 3 && normalized.ends_with('\\') {
            normalized.pop();
        }

        Some(normalized)
    }

    fn decide_pending_directory(
        pending_directory: &str,
        default_directory: &Path,
        settings_initialized: bool,
    ) -> PendingDirectoryDecision {
        if settings_initialized {
            return PendingDirectoryDecision::Clear;
        }

        let Some(custom_dir) = normalize_directory(pending_directory) else {
            return PendingDirectoryDecision::Clear;
        };
        let Some(prev_custom_dir) =
            normalize_directory(&default_directory.to_string_lossy())
        else {
            return PendingDirectoryDecision::Clear;
        };

        if custom_dir.eq_ignore_ascii_case(&prev_custom_dir) {
            return PendingDirectoryDecision::Clear;
        }

        PendingDirectoryDecision::Apply {
            custom_dir,
            prev_custom_dir,
        }
    }

    /// Finds the pending directory an installer recorded, if any.
    ///
    /// Every key is tried and the first one that actually *has* the value wins,
    /// because the installer creates its own key on every run: stopping at the
    /// first openable key would make the legacy fallback unreachable. A key that
    /// cannot be read is stepped over so one restrictive ACL does not hide a
    /// value recorded by the other installer.
    ///
    /// Nothing here is worth failing the launch over: a machine whose registry
    /// cannot be read simply keeps its configured directory, which is the same
    /// result as an install that never recorded one.
    fn find_pending_directory() -> Option<String> {
        for path in INSTALLER_REGISTRY_KEYS {
            let Ok(key) = RegKey::predef(HKEY_CURRENT_USER)
                .open_subkey_with_flags(path, KEY_READ)
            else {
                continue;
            };
            match key.get_value(PENDING_RESOURCE_DIRECTORY_VALUE) {
                Ok(value) => return Some(value),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::warn!(
                        key = path,
                        %error,
                        "Could not read the installer's pending directory"
                    );
                }
            }
        }
        None
    }

    /// Removes the recorded value so it is only applied once.
    ///
    /// The lookup opens keys for reading, so write access is requested here
    /// rather than for every launch. A key that cannot be written leaves the
    /// value in place, which costs one redundant re-apply instead of failing the
    /// startup that already succeeded.
    fn clear_pending_directory() {
        for path in INSTALLER_REGISTRY_KEYS {
            let Ok(key) = RegKey::predef(HKEY_CURRENT_USER)
                .open_subkey_with_flags(path, KEY_SET_VALUE)
            else {
                continue;
            };
            match key.delete_value(PENDING_RESOURCE_DIRECTORY_VALUE) {
                Ok(()) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::warn!(
                        key = path,
                        %error,
                        "Could not clear the installer's pending directory"
                    );
                    return;
                }
            }
        }
    }

    pub async fn apply_pending_installer_directory(
        settings: &mut Settings,
        pool: &SqlitePool,
        app_identifier: &str,
    ) -> crate::Result<()> {
        if std::env::var_os("THESEUS_CONFIG_DIR").is_some() {
            return Ok(());
        }

        let Some(pending_directory) = find_pending_directory() else {
            return Ok(());
        };
        let default_directory = DirectoryInfo::initial_settings_dir_path(
            app_identifier,
        )
        .ok_or(crate::ErrorKind::FSError(
            "Could not find valid config dir".to_string(),
        ))?;
        let settings_initialized =
            settings.custom_dir.is_some() || settings.prev_custom_dir.is_some();

        match decide_pending_directory(
            &pending_directory,
            &default_directory,
            settings_initialized,
        ) {
            PendingDirectoryDecision::Apply {
                custom_dir,
                prev_custom_dir,
            } => {
                tracing::info!(
                    "Applying the application directory selected by the installer"
                );
                settings.custom_dir = Some(custom_dir);
                settings.prev_custom_dir = Some(prev_custom_dir);
                settings.update(pool).await?;
                clear_pending_directory();
            }
            PendingDirectoryDecision::Clear => {
                clear_pending_directory();
            }
        }

        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn default_directory() -> PathBuf {
            PathBuf::from(r"C:\Users\Test\AppData\Roaming\red.ghs.axolotl")
        }

        #[test]
        fn the_lookup_checks_the_key_this_installer_writes_first() {
            // The NSIS script derives its key from the bundle's manufacturer and
            // product name. Reading only the upstream key meant the data folder
            // chosen at install time was never applied, which is what left
            // instances on the system drive.
            assert_eq!(INSTALLER_REGISTRY_KEYS[0], r"Software\mctrin\MC-Trin");
            assert!(
                INSTALLER_REGISTRY_KEYS
                    .contains(&r"Software\ghs\Axolotl Launcher"),
                "older installs must keep resolving"
            );
        }

        #[test]
        fn applies_a_custom_installer_directory() {
            assert_eq!(
                decide_pending_directory(
                    r"D:\Minecraft\Axolotl",
                    &default_directory(),
                    false,
                ),
                PendingDirectoryDecision::Apply {
                    custom_dir: r"D:\Minecraft\Axolotl".to_string(),
                    prev_custom_dir: default_directory()
                        .to_string_lossy()
                        .to_string(),
                }
            );
        }

        #[test]
        fn clears_a_default_installer_directory() {
            assert_eq!(
                decide_pending_directory(
                    r"C:\Users\Test\AppData\Roaming\red.ghs.axolotl\",
                    &default_directory(),
                    false,
                ),
                PendingDirectoryDecision::Clear
            );
        }

        #[test]
        fn does_not_override_existing_settings() {
            assert_eq!(
                decide_pending_directory(
                    r"D:\Minecraft\Axolotl",
                    &default_directory(),
                    true,
                ),
                PendingDirectoryDecision::Clear
            );
        }

        #[test]
        fn clears_invalid_relative_and_root_directories() {
            let default_directory = default_directory();
            for path in ["relative", r"C:\", ""] {
                assert_eq!(
                    decide_pending_directory(path, &default_directory, false),
                    PendingDirectoryDecision::Clear
                );
            }
        }

        #[test]
        fn applied_state_can_restore_the_default_directory() {
            let PendingDirectoryDecision::Apply {
                custom_dir: _,
                prev_custom_dir,
            } = decide_pending_directory(
                r"D:\Minecraft\Axolotl",
                &default_directory(),
                false,
            )
            else {
                panic!("expected installer directory to be applied");
            };

            assert_eq!(prev_custom_dir, default_directory().to_string_lossy());
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows::apply_pending_installer_directory;

#[cfg(not(target_os = "windows"))]
pub async fn apply_pending_installer_directory(
    _settings: &mut super::Settings,
    _pool: &sqlx::SqlitePool,
    _app_identifier: &str,
) -> crate::Result<()> {
    Ok(())
}
