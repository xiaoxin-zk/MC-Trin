use tauri::plugin::TauriPlugin;
use theseus::prelude::UserFriend;

pub fn init<R: tauri::Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new("friends")
        .invoke_handler(tauri::generate_handler![
            friends,
            add_friend,
            remove_friend
        ])
        .build()
}

#[tauri::command]
pub async fn friends() -> crate::api::Result<Vec<UserFriend>> {
    Ok(theseus::friends::friends().await?)
}

#[tauri::command]
pub async fn add_friend(user_id: &str) -> crate::api::Result<()> {
    Ok(theseus::friends::add_friend(user_id).await?)
}

#[tauri::command]
pub async fn remove_friend(user_id: &str) -> crate::api::Result<()> {
    Ok(theseus::friends::remove_friend(user_id).await?)
}
