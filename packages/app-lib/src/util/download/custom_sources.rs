use serde::{Deserialize, Serialize};
use url::Url;

const MAX_CUSTOM_DOWNLOAD_SOURCES: usize = 20;
const MAX_CUSTOM_SOURCE_ID_LENGTH: usize = 64;
const MAX_CUSTOM_SOURCE_NAME_LENGTH: usize = 64;
const MAX_CUSTOM_SOURCE_URL_LENGTH: usize = 512;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomDownloadSourceKind {
    Minecraft,
    Modrinth,
    Curseforge,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CustomDownloadSource {
    pub id: String,
    pub name: String,
    pub kind: CustomDownloadSourceKind,
    pub base_url: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: u32,
}

fn default_enabled() -> bool {
    true
}

pub fn normalize_base_url(input: &str) -> crate::Result<String> {
    let input = input.trim();
    if input.len() > MAX_CUSTOM_SOURCE_URL_LENGTH {
        return Err(input_error("Download source URL is too long"));
    }
    let mut url = Url::parse(input)
        .map_err(|_| input_error("Download source URL is invalid"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(input_error(
            "Download source must be an HTTPS base URL without credentials, query, or fragment",
        ));
    }
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

pub fn normalize_download_sources(
    sources: &mut [CustomDownloadSource],
) -> crate::Result<()> {
    if sources.len() > MAX_CUSTOM_DOWNLOAD_SOURCES {
        return Err(input_error("Too many custom download sources"));
    }

    let mut ids = std::collections::HashSet::new();
    let mut endpoints = std::collections::HashSet::new();
    for source in sources.iter_mut() {
        source.id = source.id.trim().to_string();
        if source.id.is_empty()
            || source.id.len() > MAX_CUSTOM_SOURCE_ID_LENGTH
            || !source.id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
            })
            || !ids.insert(source.id.clone())
        {
            return Err(input_error(
                "Download source ID is invalid or duplicated",
            ));
        }

        source.name = source.name.trim().to_string();
        if source.name.is_empty()
            || source.name.chars().count() > MAX_CUSTOM_SOURCE_NAME_LENGTH
        {
            return Err(input_error("Download source name is invalid"));
        }

        source.base_url = normalize_base_url(&source.base_url)?;
        if !endpoints.insert((source.kind, source.base_url.clone())) {
            return Err(input_error("Download source is duplicated"));
        }
    }

    sources.sort_by_key(|source| source.priority);
    for (priority, source) in sources.iter_mut().enumerate() {
        source.priority = priority as u32;
    }
    Ok(())
}

fn input_error(message: &str) -> crate::Error {
    crate::ErrorKind::InputError(message.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str, priority: u32, base_url: &str) -> CustomDownloadSource {
        CustomDownloadSource {
            id: id.to_string(),
            name: id.to_string(),
            kind: CustomDownloadSourceKind::Modrinth,
            base_url: base_url.to_string(),
            enabled: true,
            priority,
        }
    }

    #[test]
    fn accepts_and_normalizes_https_base_urls() {
        assert_eq!(
            normalize_base_url(" https://mirror.example/modrinth/// ").unwrap(),
            "https://mirror.example/modrinth"
        );
    }

    #[test]
    fn rejects_unsafe_or_ambiguous_urls() {
        for value in [
            "http://mirror.example",
            "ftp://mirror.example/files",
            "https://user:pass@mirror.example",
            "https://mirror.example/path?token=secret",
            "https://mirror.example/path#section",
            "not a URL",
        ] {
            assert!(normalize_base_url(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn sorts_sources_by_priority_and_rewrites_stable_priorities() {
        let mut sources = vec![
            source("late", 9, "https://late.example"),
            source("first", 0, "https://first.example"),
            source("middle", 4, "https://middle.example"),
        ];

        normalize_download_sources(&mut sources).unwrap();

        assert_eq!(
            sources
                .iter()
                .map(|source| source.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "middle", "late"]
        );
        assert_eq!(
            sources
                .iter()
                .map(|source| source.priority)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }
}
