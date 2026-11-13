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
    Ok(mctrin::hongshi::download()
        .await
        .map_err(mctrin::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_get_state()
-> Result<mctrin::multiplayer::MultiplayerState> {
    Ok(mctrin::multiplayer::get_state().await)
}

#[tauri::command]
pub async fn multiplayer_get_nodes(
    force_refresh: Option<bool>,
) -> Result<Vec<mctrin::hongshi::HongshiNode>> {
    Ok(mctrin::hongshi::get_nodes(force_refresh.unwrap_or(false))
        .await
        .map_err(mctrin::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_get_detected_ports()
-> Result<Vec<mctrin::hongshi::DetectedLanPort>> {
    Ok(mctrin::hongshi::get_detected_ports().await)
}

#[tauri::command]
pub async fn multiplayer_switch_provider(
    provider: mctrin::multiplayer::MultiplayerProvider,
) -> Result<()> {
    Ok(mctrin::multiplayer::switch_provider(provider)
        .await
        .map_err(mctrin::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_prepare_terracotta() -> Result<()> {
    Ok(mctrin::multiplayer::prepare_terracotta()
        .await
        .map_err(mctrin::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_start_mctier<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    request: mctrin::mctier::StartRequest,
) -> Result<()> {
    use tauri::{Emitter, Manager};

    let voice_signal_emitter =
        std::sync::Arc::new(move |signal: serde_json::Value| {
            if let Some(main_window) = app.get_webview_window("main") {
                let _ = main_window.emit("mctier-voice-signal", signal);
            }
        });
    Ok(
        mctrin::multiplayer::start_mctier(request, voice_signal_emitter)
            .await
            .map_err(mctrin::Error::from)?,
    )
}

#[tauri::command]
pub async fn multiplayer_start_direct(
    request: mctrin::mctier::DirectRequest,
) -> Result<()> {
    Ok(mctrin::multiplayer::start_direct(request)
        .await
        .map_err(mctrin::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_mctier_send_voice_signal(
    signal: serde_json::Value,
) -> Result<()> {
    Ok(mctrin::mctier::send_voice_signal(signal)
        .await
        .map_err(mctrin::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_mctier_set_public(is_public: bool) -> Result<()> {
    Ok(mctrin::mctier::set_public(is_public)
        .await
        .map_err(mctrin::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_mctier_create_invite() -> Result<String> {
    Ok(mctrin::mctier::create_invite()
        .await
        .map_err(mctrin::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_host(
    request: mctrin::multiplayer::MultiplayerHostRequest,
) -> Result<()> {
    Ok(mctrin::multiplayer::host(request)
        .await
        .map_err(mctrin::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_join(
    request: mctrin::multiplayer::MultiplayerJoinRequest,
) -> Result<()> {
    Ok(mctrin::multiplayer::join(request)
        .await
        .map_err(mctrin::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_stop() -> Result<()> {
    Ok(mctrin::multiplayer::stop()
        .await
        .map_err(mctrin::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_reset() -> Result<()> {
    Ok(mctrin::multiplayer::reset()
        .await
        .map_err(mctrin::Error::from)?)
}

#[tauri::command]
pub async fn multiplayer_get_player_name() -> Result<String> {
    Ok(mctrin::terracotta::get_player_name().await)
}

#[tauri::command]
pub async fn multiplayer_open_hongshi_logs<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<()> {
    tokio::fs::create_dir_all(mctrin::hongshi::logs_dir()).await?;
    crate::api::utils::open_path(app, mctrin::hongshi::logs_dir()).await;
    Ok(())
}
