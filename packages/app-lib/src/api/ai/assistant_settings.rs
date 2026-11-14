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
    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT provider_id, model_id FROM assistant_ai_settings WHERE id = 0",
    )
    .fetch_one(&state.pool)
    .await?;
    Ok(AssistantAiSettings {
        provider_id: row.0,
        model_id: row.1,
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
        "UPDATE assistant_ai_settings SET provider_id = ?, model_id = ? WHERE id = 0",
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
}
