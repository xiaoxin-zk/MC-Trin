#![allow(clippy::should_implement_trait)]

use crate::state::ModLoader;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::unknown_value;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentSourceKind {
    Local,
    CurseForge,
    McArchive,
    ModrinthModpack,
    ServerProject,
    ImportedModpack,
    SharedInstance,
}

impl ContentSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::CurseForge => "curseforge",
            Self::McArchive => "mcarchive",
            Self::ModrinthModpack => "modrinth_modpack",
            Self::ServerProject => "server_project",
            Self::ImportedModpack => "imported_modpack",
            Self::SharedInstance => "shared_instance",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentSetStatus {
    Available,
    Installing,
    Stale,
    MissingFiles,
}

impl ContentSetStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Installing => "installing",
            Self::Stale => "stale",
            Self::MissingFiles => "missing_files",
        }
    }
}

/// Represents a playable setup slot for an instance.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentSet {
    pub id: String,
    pub instance_id: String,
    pub name: String,
    pub source_kind: ContentSourceKind,
    pub status: ContentSetStatus,
    pub game_version: String,
    pub protocol_version: Option<u32>,
    pub loader: ModLoader,
    pub loader_version: Option<String>,
    #[serde(default)]
    pub revision: u64,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
}

impl std::str::FromStr for ContentSourceKind {
    type Err = crate::Error;

    fn from_str(value: &str) -> crate::Result<Self> {
        match value {
            "local" => Ok(Self::Local),
            "curseforge" => Ok(Self::CurseForge),
            "mcarchive" => Ok(Self::McArchive),
            "modrinth_modpack" => Ok(Self::ModrinthModpack),
            "server_project" => Ok(Self::ServerProject),
            "imported_modpack" => Ok(Self::ImportedModpack),
            "shared_instance" => Ok(Self::SharedInstance),
            other => Err(unknown_value("content source kind", other)),
        }
    }
}

impl std::str::FromStr for ContentSetStatus {
    type Err = crate::Error;

    fn from_str(value: &str) -> crate::Result<Self> {
        match value {
            "available" => Ok(Self::Available),
            "installing" => Ok(Self::Installing),
            "stale" => Ok(Self::Stale),
            "missing_files" => Ok(Self::MissingFiles),
            other => Err(unknown_value("content set status", other)),
        }
    }
}

impl ContentSourceKind {
    pub fn from_str(value: &str) -> crate::Result<Self> {
        <Self as std::str::FromStr>::from_str(value)
    }
}

impl ContentSetStatus {
    pub fn from_str(value: &str) -> crate::Result<Self> {
        <Self as std::str::FromStr>::from_str(value)
    }
}
