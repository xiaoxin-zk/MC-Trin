mod codec;
mod limits;
mod modpack;
mod operations;
mod projection;
mod reconciliation;
mod storage;
mod types;

const SERVERS_FILE: &str = "servers.dat";

pub(crate) use self::codec::{
    read_pack_servers, read_pack_servers_async, server_data,
};
pub(in crate::api::instance) use self::modpack::{
    prepare_modpack_servers, write_modpack_servers,
};
pub(crate) use self::operations::{
    add_user_server, ensure_managed_server, remove_server_by_index,
    update_server_by_index, world_server_sync_ids,
};
pub use self::operations::{
    desync_server, list_synced_servers, remove_synced_server,
    update_synced_server,
};
pub(super) use self::operations::{
    detach_servers, ensure_servers, merge_servers_from_instance,
    reconcile_servers, seed_servers,
};
pub(super) use self::storage::canonical_exists;
pub use self::types::{DesyncServerMode, ServerSource, SyncedServer};
