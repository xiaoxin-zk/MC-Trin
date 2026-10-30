use crate::api::Result;

pub fn init<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("multiplayer")
        .invoke_handler(tauri::generate_handler![
            multiplayer_get_state,
            multiplayer_get_nodes,
            multiplayer_get_detected_ports,
            multiplayer_download_hongshi,
            multiplayer_switch_provider,
            multiplayer_prepare_terracotta,
            multiplayer_start_mctier,
            multiplayer_start_direct,
            multiplayer_mctier_send_voice_signal,
            multiplayer_mctier_set_public,
            multiplayer_mctier_create_invite,
            multiplayer_host,
            multiplayer_join,
            multiplayer_stop,
            multiplayer_reset,
            multiplayer_get_player_name,
            multiplayer_open_hongshi_logs,
        ])
        .build()
}

#[tauri::command]
pub async fn multiplayer_download_hongshi() -> Result<()> {
    Ok(theseus::hongshi::download()
        .await
        .map_err(theseus::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_get_state()
-> Result<theseus::multiplayer::MultiplayerState> {
    Ok(theseus::multiplayer::get_state().await)
}

#[tauri::command]
pub async fn multiplayer_get_nodes(
    force_refresh: Option<bool>,
) -> Result<Vec<theseus::hongshi::HongshiNode>> {
    Ok(theseus::hongshi::get_nodes(force_refresh.unwrap_or(false))
        .await
        .map_err(theseus::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_get_detected_ports()
-> Result<Vec<theseus::hongshi::DetectedLanPort>> {
    Ok(theseus::hongshi::get_detected_ports().await)
}

#[tauri::command]
pub async fn multiplayer_switch_provider(
    provider: theseus::multiplayer::MultiplayerProvider,
) -> Result<()> {
    Ok(theseus::multiplayer::switch_provider(provider)
        .await
        .map_err(theseus::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_prepare_terracotta() -> Result<()> {
    Ok(theseus::multiplayer::prepare_terracotta()
        .await
        .map_err(theseus::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_start_mctier<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    request: theseus::mctier::StartRequest,
) -> Result<()> {
    use tauri::{Emitter, Manager};

    let voice_signal_emitter =
        std::sync::Arc::new(move |signal: serde_json::Value| {
            if let Some(main_window) = app.get_webview_window("main") {
                let _ = main_window.emit("mctier-voice-signal", signal);
            }
        });
    Ok(
        theseus::multiplayer::start_mctier(request, voice_signal_emitter)
            .await
            .map_err(theseus::Error::from)?,
    )
}

#[tauri::command]
pub async fn multiplayer_start_direct(
    request: theseus::mctier::DirectRequest,
) -> Result<()> {
    Ok(theseus::multiplayer::start_direct(request)
        .await
        .map_err(theseus::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_mctier_send_voice_signal(
    signal: serde_json::Value,
) -> Result<()> {
    Ok(theseus::mctier::send_voice_signal(signal)
        .await
        .map_err(theseus::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_mctier_set_public(is_public: bool) -> Result<()> {
    Ok(theseus::mctier::set_public(is_public)
        .await
        .map_err(theseus::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_mctier_create_invite() -> Result<String> {
    Ok(theseus::mctier::create_invite()
        .await
        .map_err(theseus::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_host(
    request: theseus::multiplayer::MultiplayerHostRequest,
) -> Result<()> {
    Ok(theseus::multiplayer::host(request)
        .await
        .map_err(theseus::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_join(
    request: theseus::multiplayer::MultiplayerJoinRequest,
) -> Result<()> {
    Ok(theseus::multiplayer::join(request)
        .await
        .map_err(theseus::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_stop() -> Result<()> {
    Ok(theseus::multiplayer::stop()
        .await
        .map_err(theseus::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_reset() -> Result<()> {
    Ok(theseus::multiplayer::reset()
        .await
        .map_err(theseus::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_get_player_name() -> Result<String> {
    Ok(theseus::terracotta::get_player_name().await)
}

#[tauri::command]
pub async fn multiplayer_open_hongshi_logs<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<()> {
    tokio::fs::create_dir_all(theseus::hongshi::logs_dir()).await?;
    crate::api::utils::open_path(app, theseus::hongshi::logs_dir()).await;
    Ok(())
}
