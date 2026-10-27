#![allow(clippy::should_implement_trait)]

use serde::{Deserialize, Serialize};

use super::unknown_value;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentSetRemoteRefType {
    SharedContentSet,
    HostingInstance,
}

impl ContentSetRemoteRefType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SharedContentSet => "shared_content_set",
            Self::HostingInstance => "hosting_instance",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentSetRemoteRef {
    pub content_set_id: String,
    pub ref_type: ContentSetRemoteRefType,
    pub ref_id: String,
}

impl std::str::FromStr for ContentSetRemoteRefType {
    type Err = crate::Error;

    fn from_str(value: &str) -> crate::Result<Self> {
        match value {
            "shared_content_set" => Ok(Self::SharedContentSet),
            "hosting_instance" => Ok(Self::HostingInstance),
            other => Err(unknown_value("content set remote ref type", other)),
        }
    }
}

impl ContentSetRemoteRefType {
    pub fn from_str(value: &str) -> crate::Result<Self> {
        <Self as std::str::FromStr>::from_str(value)
    }
}
