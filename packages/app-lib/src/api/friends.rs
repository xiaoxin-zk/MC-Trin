use crate::state::UserFriend;

#[tracing::instrument]
pub async fn friends() -> crate::Result<Vec<UserFriend>> {
    let state = crate::State::get().await?;
    crate::state::friends(&state.pool, &state.api_semaphore).await
}

#[tracing::instrument]
pub async fn add_friend(user_id: &str) -> crate::Result<()> {
    let state = crate::State::get().await?;
    crate::state::add_friend(user_id, &state.pool, &state.api_semaphore).await
}

#[tracing::instrument]
pub async fn remove_friend(user_id: &str) -> crate::Result<()> {
    let state = crate::State::get().await?;
    crate::state::remove_friend(user_id, &state.pool, &state.api_semaphore)
        .await
}
