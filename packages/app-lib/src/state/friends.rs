use crate::ErrorKind;
use crate::util::fetch::{
    FetchSemaphore, fetch_advanced, fetch_advanced_with_client, fetch_json,
};
use chrono::{DateTime, Utc};
use reqwest::Method;
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

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
    mutate_friend(
        Method::POST,
        user_id,
        env!("MODRINTH_API_URL_V3"),
        None,
        None,
        semaphore,
        exec,
    )
    .await
}

#[tracing::instrument(skip(exec, semaphore))]
pub async fn remove_friend(
    user_id: &str,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite> + Copy,
    semaphore: &FetchSemaphore,
) -> crate::Result<()> {
    mutate_friend(
        Method::DELETE,
        user_id,
        env!("MODRINTH_API_URL_V3"),
        None,
        None,
        semaphore,
        exec,
    )
    .await
}

async fn mutate_friend(
    method: Method,
    user_id: &str,
    api_base: &str,
    header: Option<(&str, &str)>,
    client: Option<&reqwest::Client>,
    semaphore: &FetchSemaphore,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite>,
) -> crate::Result<()> {
    let url = friend_endpoint(api_base, user_id)?;
    let result = match client {
        Some(client) => {
            fetch_advanced_with_client(
                method.clone(),
                &url,
                None,
                None,
                header,
                None,
                None,
                Some("/v3/friend/:user_id"),
                semaphore,
                exec,
                client,
            )
            .await
        }
        None => {
            fetch_advanced(
                method.clone(),
                &url,
                None,
                None,
                header,
                None,
                None,
                Some("/v3/friend/:user_id"),
                semaphore,
                exec,
            )
            .await
        }
    };

    if method == Method::POST
        && let Err(ref error) = result
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

fn friend_endpoint(api_base: &str, user_id: &str) -> crate::Result<String> {
    let mut url = url::Url::parse(api_base)?;
    url.path_segments_mut()
        .map_err(|_| ErrorKind::InputError("Invalid friends API URL".into()))?
        .pop_if_empty()
        .push("friend")
        .push(user_id);
    Ok(url.to_string())
}
