//! The model the launcher assistant uses.
//!
//! Kept apart from `crash_analysis_ai_settings` (log diagnosis) and
//! `translation_settings` (mod translation): each feature owns its own pair, so
//! changing one never silently re-points another at a different provider.

use serde::{Deserialize, Serialize};

use crate::State;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct AssistantAiSettings {
    pub provider_id: String,
    pub model_id: String,
}

pub async fn get_assistant_ai_settings() -> crate::Result<AssistantAiSettings> {
    let state = State::get().await?;
    read_assistant_ai_settings(&state.pool).await
}

/// `fetch_optional`: a database repaired by hand (or downgraded and re-upgraded)
/// can lack the seeded row, and that must read as "not chosen" rather than break
/// the assistant page.
async fn read_assistant_ai_settings(
    pool: &sqlx::SqlitePool,
) -> crate::Result<AssistantAiSettings> {
    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT provider_id, model_id FROM assistant_ai_settings WHERE id = 0",
    )
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some((provider_id, model_id)) => AssistantAiSettings {
            provider_id,
            model_id,
        },
        None => AssistantAiSettings::default(),
    })
}

pub async fn update_assistant_ai_settings(
    settings: AssistantAiSettings,
) -> crate::Result<()> {
    let state = State::get().await?;
    save_assistant_ai_settings(&state.pool, settings).await
}

async fn save_assistant_ai_settings(
    pool: &sqlx::SqlitePool,
    settings: AssistantAiSettings,
) -> crate::Result<()> {
    let provider_id = settings.provider_id.trim();
    let model_id = settings.model_id.trim();
    // A half-filled pair is not a usable choice: store neither half, so the
    // page falls back to its automatic pick instead of sending an empty model.
    let (provider_id, model_id) =
        if provider_id.is_empty() || model_id.is_empty() {
            ("", "")
        } else {
            (provider_id, model_id)
        };
    sqlx::query(
        "INSERT INTO assistant_ai_settings (id, provider_id, model_id) VALUES (0, ?, ?)
         ON CONFLICT(id) DO UPDATE SET provider_id = excluded.provider_id, model_id = excluded.model_id",
    )
    .bind(provider_id)
    .bind(model_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE assistant_ai_settings (
                id INTEGER PRIMARY KEY CHECK (id = 0),
                provider_id TEXT NOT NULL DEFAULT '',
                model_id TEXT NOT NULL DEFAULT ''
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO assistant_ai_settings (id) VALUES (0)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn saves_and_reads_the_assistant_pair_independently() {
        let pool = pool().await;
        save_assistant_ai_settings(
            &pool,
            AssistantAiSettings {
                provider_id: " openai ".into(),
                model_id: " gpt-4o-mini ".into(),
            },
        )
        .await
        .unwrap();
        let row: (String, String) = sqlx::query_as(
            "SELECT provider_id, model_id FROM assistant_ai_settings WHERE id = 0",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row, ("openai".into(), "gpt-4o-mini".into()));
    }

    #[tokio::test]
    async fn a_half_filled_choice_is_stored_as_no_choice() {
        let pool = pool().await;
        save_assistant_ai_settings(
            &pool,
            AssistantAiSettings {
                provider_id: "openai".into(),
                model_id: "   ".into(),
            },
        )
        .await
        .unwrap();
        let row: (String, String) = sqlx::query_as(
            "SELECT provider_id, model_id FROM assistant_ai_settings WHERE id = 0",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row, (String::new(), String::new()));
    }

    /// A repaired database can lack the seeded row. Reading it must mean "no
    /// choice yet", not an error that takes the assistant page down.
    #[tokio::test]
    async fn a_missing_row_reads_as_no_choice() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE assistant_ai_settings (
                id INTEGER PRIMARY KEY CHECK (id = 0),
                provider_id TEXT NOT NULL DEFAULT '',
                model_id TEXT NOT NULL DEFAULT ''
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let missing = read_assistant_ai_settings(&pool).await.unwrap();
        assert_eq!(missing, AssistantAiSettings::default());

        // The insert path repairs the row rather than silently doing nothing.
        save_assistant_ai_settings(
            &pool,
            AssistantAiSettings {
                provider_id: "openai".into(),
                model_id: "gpt-4o-mini".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            read_assistant_ai_settings(&pool).await.unwrap(),
            AssistantAiSettings {
                provider_id: "openai".into(),
                model_id: "gpt-4o-mini".into(),
            }
        );
    }

    /// The assistant pair lives in its own table, so saving it cannot touch the
    /// crash-analysis pair that drives log diagnosis.
    #[tokio::test]
    async fn saving_the_assistant_pair_leaves_crash_analysis_alone() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE crash_analysis_ai_settings (
                id INTEGER PRIMARY KEY CHECK (id = 0),
                enabled INTEGER NOT NULL DEFAULT FALSE,
                provider_id TEXT NOT NULL DEFAULT '',
                model_id TEXT NOT NULL DEFAULT '',
                ai_source TEXT NOT NULL DEFAULT 'logshare'
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO crash_analysis_ai_settings VALUES (0, 1, 'openai', 'crash-model', 'custom')",
        )
        .execute(&pool)
        .await
        .unwrap();

        save_assistant_ai_settings(
            &pool,
            AssistantAiSettings {
                provider_id: "deepseek".into(),
                model_id: "deepseek-chat".into(),
            },
        )
        .await
        .unwrap();

        let crash: (i64, String, String, String) = sqlx::query_as(
            "SELECT enabled, provider_id, model_id, ai_source
             FROM crash_analysis_ai_settings WHERE id = 0",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            crash,
            (1, "openai".into(), "crash-model".into(), "custom".into())
        );
    }
}
