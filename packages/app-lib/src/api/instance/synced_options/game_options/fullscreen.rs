//! Applies the synchronized fullscreen option to the app settings.

use crate::state::CanonicalValue;
use sqlx::{Sqlite, Transaction};

pub(super) async fn update_app_fullscreen_setting(
    tx: &mut Transaction<'_, Sqlite>,
    value: &CanonicalValue,
    sync_enabled: bool,
) -> crate::Result<()> {
    if !sync_enabled {
        return Ok(());
    }
    let CanonicalValue::Bool(value) = value else {
        return Ok(());
    };
    sqlx::query("UPDATE settings SET mc_force_fullscreen = ? WHERE id = 0")
        .bind(*value)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
