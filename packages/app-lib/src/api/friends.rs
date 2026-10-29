use crate::state::{FriendsSocket, UserFriend};
use ariadne::users::UserStatus;

#[tracing::instrument]
pub async fn friends() -> crate::Result<Vec<UserFriend>> {
    let state = crate::State::get().await?;
    let friends =
        FriendsSocket::friends(&state.pool, &state.api_semaphore).await?;

    Ok(friends)
}

pub async fn friend_statuses() -> crate::Result<Vec<UserStatus>> {
    let state = crate::State::get().await?;
    let statuses = state.friends_socket.friend_statuses();

    Ok(statuses)
}

#[tracing::instrument]
pub async fn add_friend(user_id: &str) -> crate::Result<()> {
    let state = crate::State::get().await?;
    FriendsSocket::add_friend(user_id, &state.pool, &state.api_semaphore)
        .await?;

    Ok(())
}

#[tracing::instrument]
pub async fn remove_friend(user_id: &str) -> crate::Result<()> {
    let state = crate::State::get().await?;
    FriendsSocket::remove_friend(user_id, &state.pool, &state.api_semaphore)
        .await?;

    Ok(())
}

/// Exposes one local Minecraft port to friends over the friend tunnel.
///
/// Returns the port now being shared so the caller can show it to the user.
#[tracing::instrument]
pub async fn share_port(port: u16) -> crate::Result<u16> {
    let state = crate::State::get().await?;
    if !state.friends_socket.is_connected().await {
        return Err(crate::ErrorKind::InputError(
            "Sharing a port needs an active friends connection".to_string(),
        )
        .as_error());
    }

    state.friends_socket.share_port(port).await
}

/// Withdraws every port currently shared with friends and returns how many
/// were closed.
#[tracing::instrument]
pub async fn stop_sharing_ports() -> crate::Result<usize> {
    let state = crate::State::get().await?;
    state.friends_socket.stop_shared_ports().await
}
