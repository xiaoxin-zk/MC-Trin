use serde::{Deserialize, Serialize};

use crate::State;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CrashAnalysisAiSettings {
    pub enabled: bool,
    pub provider_id: String,
    pub model_id: String,
    pub ai_source: String,
}

pub async fn get_crash_analysis_ai_settings()
-> crate::Result<CrashAnalysisAiSettings> {
    let state = State::get().await?;
    let row = sqlx::query_as::<_, (i64, String, String, String)>(
        "SELECT enabled, provider_id, model_id, ai_source FROM crash_analysis_ai_settings WHERE id = 0",
    )
    .fetch_one(&state.pool)
    .await?;
    // A stored `logshare` only survives if the user explicitly consented.
    // Otherwise the row predates the opt-in (or was left by a revoke) and the
    // effective source reported to the UI must stay local.
    let ai_source = effective_ai_source(
        &row.3,
        super::logshare::remote_consent_recorded(&state.pool).await,
    );
    Ok(CrashAnalysisAiSettings {
        enabled: row.0 != 0,
        provider_id: row.1,
        model_id: row.2,
        ai_source: ai_source.to_string(),
    })
}

pub async fn update_crash_analysis_ai_settings(
    settings: CrashAnalysisAiSettings,
) -> crate::Result<()> {
    let state = State::get().await?;
    save_settings(&state.pool, settings).await
}

async fn save_settings(
    pool: &sqlx::SqlitePool,
    settings: CrashAnalysisAiSettings,
) -> crate::Result<()> {
    let ai_source = normalize_ai_source(&settings.ai_source);
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "UPDATE crash_analysis_ai_settings SET enabled = ?, provider_id = ?, model_id = ?, ai_source = ? WHERE id = 0",
    )
    .bind(settings.enabled)
    .bind(settings.provider_id.trim())
    .bind(settings.model_id.trim())
    .bind(ai_source)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE log_share_settings SET ai_source = ? WHERE id = 0")
        .bind(ai_source)
        .execute(&mut *transaction)
        .await?;
    // Selecting the remote source only records a preference. Sending logs is a
    // separate, explicit opt-in handled by `update_log_share_settings`, so a
    // settings save that merely carries an inherited value cannot consent.
    transaction.commit().await?;
    Ok(())
}

/// Only an explicit remote choice selects LogShare. Every other value, including
/// an empty or unexpected one, keeps crash analysis on this machine.
fn normalize_ai_source(ai_source: &str) -> &'static str {
    if ai_source == super::logshare::REMOTE_AI_SOURCE {
        super::logshare::REMOTE_AI_SOURCE
    } else {
        super::logshare::LOCAL_AI_SOURCE
    }
}

/// Effective source for a stored row: remote needs both the stored value and a
/// recorded consent, so a legacy row reads as local.
fn effective_ai_source(stored: &str, consent: bool) -> &'static str {
    if consent {
        normalize_ai_source(stored)
    } else {
        super::logshare::LOCAL_AI_SOURCE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_analysis_is_local_unless_the_user_explicitly_chose_logshare() {
        assert_eq!(normalize_ai_source(""), "custom");
        assert_eq!(normalize_ai_source("custom"), "custom");
        assert_eq!(normalize_ai_source("something-else"), "custom");
        assert_eq!(normalize_ai_source("logshare"), "logshare");
    }

    #[test]
    fn the_stored_settings_default_to_local_analysis() {
        let settings = super::super::logshare::LogShareSettings::default();
        assert_eq!(settings.ai_source, "custom");
        assert!(
            !settings.auto_upload,
            "no crash log may leave the machine before the user opts in"
        );
    }

    #[tokio::test]
    async fn saves_authoritative_source_and_mirror_atomically() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE crash_analysis_ai_settings (id INTEGER PRIMARY KEY, enabled BOOLEAN, provider_id TEXT, model_id TEXT, ai_source TEXT)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO crash_analysis_ai_settings VALUES (0, 0, '', '', 'custom')").execute(&pool).await.unwrap();
        let settings = CrashAnalysisAiSettings {
            enabled: true,
            provider_id: " provider ".into(),
            model_id: " model ".into(),
            ai_source: "custom".into(),
        };
        assert!(save_settings(&pool, settings.clone()).await.is_err());
        let source: String = sqlx::query_scalar(
            "SELECT ai_source FROM crash_analysis_ai_settings",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(source, "custom");
        sqlx::query("CREATE TABLE log_share_settings (id INTEGER PRIMARY KEY, ai_source TEXT)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO log_share_settings VALUES (0, 'custom')")
            .execute(&pool)
            .await
            .unwrap();
        save_settings(&pool, settings).await.unwrap();
        let row: (String, String, String, String) = sqlx::query_as("SELECT a.provider_id, a.model_id, a.ai_source, s.ai_source FROM crash_analysis_ai_settings a JOIN log_share_settings s ON s.id = a.id").fetch_one(&pool).await.unwrap();
        assert_eq!(
            row,
            (
                "provider".into(),
                "model".into(),
                "custom".into(),
                "custom".into()
            )
        );
    }
}
