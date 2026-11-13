use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use thiserror::Error;

pub mod ai;
pub mod auth;
pub mod import;
pub mod install;
pub mod instance;
pub mod jre;
pub mod logs;
pub mod mcarchive;
pub mod metadata;
pub mod minecraft_skins;
pub mod mod_translation;
pub mod mr_auth;
pub mod multiplayer;
pub mod planet_minecraft;
pub mod process;
pub mod schematic_preview;
pub mod seed_map;
pub mod settings;
pub mod shortcuts;
pub mod storage;
pub mod system_accent;
pub mod tags;
pub mod terracotta;
pub mod translation;
pub mod utils;

pub mod cache;
pub mod content_favorites;
pub mod content_search;
pub mod curseforge;
pub mod datapacks;
pub mod drop;
pub mod files;
pub mod fonts;
pub mod friends;
pub mod resources;
pub mod worlds;

mod oauth_utils;
mod search_cancellation;

pub type Result<T> = std::result::Result<T, MctrinSerializableError>;

// // Main returnable MC-Trin GUI error
// // Needs to be Serializable to be returned to the JavaScript side
// #[derive(Error, Debug, Serialize)]
// pub enum MctrinGuiError {
//     #[error(transparent)]
//     Serializable(),
// }

// Serializable error intermediary, so MctrinGuiError can be Serializable (eg: so that we can return mctrin::Errors in Tauri directly)
#[derive(Error, Debug)]
pub enum MctrinSerializableError {
    #[error("{0}")]
    Mctrin(#[from] mctrin::Error),

    #[error("IO error: {0}")]
    IO(#[from] std::io::Error),

    #[error("Tauri error: {0}")]
    Tauri(#[from] tauri::Error),

    #[cfg(feature = "updater")]
    #[error("Updater error: {0}")]
    Updater(#[from] tauri_plugin_updater::Error),

    #[cfg(feature = "updater")]
    #[error("HTTP error: {0}")]
    Http(#[from] tauri_plugin_http::reqwest::Error),

    #[error("Search request cancelled: {0}")]
    SearchCancelled(String),

    #[error("{0}")]
    Resource(#[from] mctrin::resources::ResourceError),
}

// Generic implementation of From<T> for ErrorTypeA
// impl<T> From<T> for MctrinGuiError
// where
//     MctrinSerializableError: From<T>,
// {
//     fn from(error: T) -> Self {
//         MctrinGuiError::Serializable(MctrinSerializableError::from(error))
//     }
// }

// This is a very simple macro that implements a very basic Serializable for each variant of MctrinSerializableError,
// where the field is the string. (This allows easy extension to errors without many match arms)
macro_rules! impl_serialize {
    ($($variant:ident),* $(,)?) => {
        impl Serialize for MctrinSerializableError {
            fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                match self {
                    // For the mctrin variant, we add a special display for the error,
                    // to view the spans if subscribed to them (which is information that is lost when serializing)
                    MctrinSerializableError::Mctrin(mctrin_error) => {
                        $crate::error::display_tracing_error(mctrin_error);

                        let mut state = serializer.serialize_struct("Mctrin", 2)?;
                        state.serialize_field("field_name", "Mctrin")?;
                        state.serialize_field(
                            "message",
                            &mctrin_error.user_facing_message(),
                        )?;
                        state.end()
                    }
                    MctrinSerializableError::Resource(error) => {
                        if !matches!(error, mctrin::resources::ResourceError::Canceled) {
                            tracing::error!(error = %error, "Resource operation failed");
                        }
                        let mut state = serializer.serialize_struct("Resource", 3)?;
                        state.serialize_field("field_name", "Resource")?;
                        state.serialize_field("message", &error.to_string())?;
                        state.serialize_field("code", if matches!(error, mctrin::resources::ResourceError::CursorExpired) { "cursorExpired" } else { "resourceError" })?;
                        state.end()
                    }
                    $(
                        MctrinSerializableError::$variant(message) => {
                            let mut state = serializer.serialize_struct(stringify!($variant), 2)?;
                            state.serialize_field("field_name", stringify!($variant))?;
                            state.serialize_field("message", &message.to_string())?;
                            state.end()
                        },
                    )*
                }
            }
        }
    };
}

// Use the macro to implement Serialize for MctrinSerializableError
#[cfg(not(feature = "updater"))]
impl_serialize! {
    IO,
    Tauri,
    SearchCancelled,
}

#[cfg(feature = "updater")]
impl_serialize! {
    IO,
    Tauri,
    Updater,
    Http,
    SearchCancelled,
}
