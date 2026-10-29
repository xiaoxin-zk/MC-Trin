use crate::ErrorKind;
use crate::util::fetch::{FetchSemaphore, fetch_advanced, fetch_json};
use chrono::{DateTime, Utc};
use reqwest::Method;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct UserFriend {
    pub id: String,
    pub friend_id: String,
    pub accepted: bool,
    pub created: DateTime<Utc>,
}

#[tracing::instrument(skip(exec, semaphore))]
pub async fn friends(
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite> + Copy,
    semaphore: &FetchSemaphore,
) -> crate::Result<Vec<UserFriend>> {
    fetch_json(
        Method::GET,
        concat!(env!("MODRINTH_API_URL_V3"), "friends"),
        None,
        None,
        Some("/v3/friends"),
        semaphore,
        exec,
    )
    .await
}

#[tracing::instrument(skip(exec, semaphore))]
pub async fn add_friend(
    user_id: &str,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite> + Copy,
    semaphore: &FetchSemaphore,
) -> crate::Result<()> {
    let result = fetch_advanced(
        Method::POST,
        &format!("{}friend/{user_id}", env!("MODRINTH_API_URL_V3")),
        None,
        None,
        None,
        None,
        None,
        Some("/v3/friend/:user_id"),
        semaphore,
        exec,
    )
    .await;

    if let Err(ref error) = result
        && let ErrorKind::LabrinthError(error) = &*error.raw
        && error.error == "not_found"
    {
        return Err(ErrorKind::OtherError(format!(
            "No user found with username \"{user_id}\""
        ))
        .into());
    }
    result?;
    Ok(())
}

#[tracing::instrument(skip(exec, semaphore))]
pub async fn remove_friend(
    user_id: &str,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite> + Copy,
    semaphore: &FetchSemaphore,
) -> crate::Result<()> {
    fetch_advanced(
        Method::DELETE,
        &format!("{}friend/{user_id}", env!("MODRINTH_API_URL_V3")),
        None,
        None,
        None,
        None,
        None,
        Some("/v3/friend/:user_id"),
        semaphore,
        exec,
    )
    .await?;
    Ok(())
}
