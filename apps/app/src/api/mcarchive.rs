use crate::api::Result;

pub fn init<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("mcarchive")
        .invoke_handler(tauri::generate_handler![
            mcarchive_get_game_versions,
            mcarchive_search_mods,
            mcarchive_get_mod_by_slug,
            mcarchive_get_file_by_filename,
            mcarchive_get_file_by_sha256,
        ])
        .build()
}

#[tauri::command]
pub async fn mcarchive_get_game_versions()
-> Result<Vec<mctrin::mcarchive::McArchiveGameVersion>> {
    Ok(mctrin::mcarchive::get_game_versions().await?)
}

#[tauri::command]
pub async fn mcarchive_search_mods(
    keyword: &str,
    game_version: Option<&str>,
) -> Result<Vec<mctrin::mcarchive::McArchiveMod>> {
    Ok(mctrin::mcarchive::search_mods(keyword, game_version).await?)
}

#[tauri::command]
pub async fn mcarchive_get_mod_by_slug(
    slug: &str,
) -> Result<mctrin::mcarchive::McArchiveMod> {
    Ok(mctrin::mcarchive::get_mod_by_slug(slug).await?)
}

#[tauri::command]
pub async fn mcarchive_get_file_by_filename(
    filename: &str,
) -> Result<Option<mctrin::mcarchive::McArchiveFile>> {
    Ok(mctrin::mcarchive::get_file_by_filename(filename).await?)
}

#[tauri::command]
pub async fn mcarchive_get_file_by_sha256(
    sha256: &str,
) -> Result<Option<mctrin::mcarchive::McArchiveFile>> {
    Ok(mctrin::mcarchive::get_file_by_sha256(sha256).await?)
}
