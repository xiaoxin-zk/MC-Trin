use tauri::plugin::TauriPlugin;
use theseus::prelude::{UserFriend, UserStatus};

pub fn init<R: tauri::Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new("friends")
        .invoke_handler(tauri::generate_handler![
            friends,
            friend_statuses,
            add_friend,
            remove_friend,
            share_friend_port,
            stop_sharing_friend_ports
        ])
        .build()
}

#[tauri::command]
pub async fn friends() -> crate::api::Result<Vec<UserFriend>> {
    Ok(theseus::friends::friends().await?)
}

#[tauri::command]
pub async fn friend_statuses() -> crate::api::Result<Vec<UserStatus>> {
    Ok(theseus::friends::friend_statuses().await?)
}

#[tauri::command]
pub async fn add_friend(user_id: &str) -> crate::api::Result<()> {
    Ok(theseus::friends::add_friend(user_id).await?)
}

#[tauri::command]
pub async fn remove_friend(user_id: &str) -> crate::api::Result<()> {
    Ok(theseus::friends::remove_friend(user_id).await?)
}

#[tauri::command]
pub async fn share_friend_port(port: u16) -> crate::api::Result<u16> {
    Ok(theseus::friends::share_port(port).await?)
}

#[tauri::command]
pub async fn stop_sharing_friend_ports() -> crate::api::Result<usize> {
    Ok(theseus::friends::stop_sharing_ports().await?)
}
