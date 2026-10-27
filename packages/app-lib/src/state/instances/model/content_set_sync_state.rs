#![allow(clippy::should_implement_trait)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::unknown_value;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentSetSyncProvider {
    SharedInstance,
}

impl ContentSetSyncProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SharedInstance => "shared_instance",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentSetSyncStatus {
    Unknown,
    UpToDate,
    UpdateAvailable,
    Applying,
    Stale,
    NotReady,
    Error,
}

impl ContentSetSyncStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::UpToDate => "up_to_date",
            Self::UpdateAvailable => "update_available",
            Self::Applying => "applying",
            Self::Stale => "stale",
            Self::NotReady => "not_ready",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentSetSyncState {
    pub content_set_id: String,
    pub provider: ContentSetSyncProvider,
    pub applied_update_id: Option<String>,
    pub latest_available_update_id: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
    pub status: ContentSetSyncStatus,
}

impl std::str::FromStr for ContentSetSyncProvider {
    type Err = crate::Error;

    fn from_str(value: &str) -> crate::Result<Self> {
        match value {
            "shared_instance" => Ok(Self::SharedInstance),
            other => Err(unknown_value("content set sync provider", other)),
        }
    }
}

impl std::str::FromStr for ContentSetSyncStatus {
    type Err = crate::Error;

    fn from_str(value: &str) -> crate::Result<Self> {
        match value {
            "unknown" => Ok(Self::Unknown),
            "up_to_date" => Ok(Self::UpToDate),
            "update_available" => Ok(Self::UpdateAvailable),
            "applying" => Ok(Self::Applying),
            "stale" => Ok(Self::Stale),
            "not_ready" => Ok(Self::NotReady),
            "error" => Ok(Self::Error),
            other => Err(unknown_value("content set sync status", other)),
        }
    }
}

impl ContentSetSyncProvider {
    pub fn from_str(value: &str) -> crate::Result<Self> {
        <Self as std::str::FromStr>::from_str(value)
    }
}

impl ContentSetSyncStatus {
    pub fn from_str(value: &str) -> crate::Result<Self> {
        <Self as std::str::FromStr>::from_str(value)
    }
}
