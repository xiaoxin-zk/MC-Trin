use super::*;
use crate::state::{CustomDownloadSource, CustomDownloadSourceKind};
use crate::util::download::constants::{
    ALIYUN_MAVEN_BASE_URL, BMCLAPI_BASE_URL, CURSEFORGE_API_HOST,
    CURSEFORGE_EDGE_HOST, FETCH_BMCLAPI2_BANGBANG93_COM,
    FETCH_FILES_MINECRAFTFORGE_NET, FETCH_LAUNCHER_MOJANG_COM,
    FETCH_LIBRARIES_MINECRAFT_NET, FETCH_MAVEN_ALIYUN_COM, FETCH_MAVEN2,
    FETCH_META_FABRICMC_NET, FETCH_MOD_MCIMIRROR_TOP,
    FETCH_PISTON_DATA_MOJANG_COM, FETCH_RELEASES, FETCH_REPO_MAVEN_APACHE_ORG,
    FETCH_REPO1_MAVEN_ORG, FETCH_RESOURCES_DOWNLOAD_MINECRAFT_NET,
    MODRINTH_CDN_HOST, RETIRED_SOURCES_MEDIA_FORGECDN_NET,
    ROUTE_POLICY_LAUNCHERMETA_MOJANG_COM, ROUTE_POLICY_MAVEN_FABRICMC_NET,
    ROUTE_POLICY_MAVEN_MINECRAFTFORGE_NET, ROUTE_POLICY_MAVEN_NEOFORGED_NET,
    ROUTE_POLICY_PISTON_META_MOJANG_COM,
};
use crate::util::download::route_policy;

pub(super) fn is_mrpack_url(url: &str) -> bool {
    reqwest::Url::parse(url).ok().is_some_and(|url| {
        url.path().to_ascii_lowercase().ends_with(FETCH_MRPACK)
    })
}

pub(super) fn route(
    url: String,
    source: DownloadRouteSource,
    is_mirror: bool,
    supports_range: bool,
) -> DownloadRoute {
    DownloadRoute {
        url,
        source,
        is_mirror,
        allow_sensitive_headers: !is_mirror,
        supports_range,
        proxy: ProxyPolicy::System,
    }
}

pub(super) fn official_route(
    url: &str,
    resource: ResourceClass,
) -> DownloadRoute {
    let url = modrinth_cdn::normalize(url).into_owned();
    let parsed_url = Url::parse(&url).ok();
    let source = parsed_url
        .as_ref()
        .and_then(|url| url.host_str().map(str::to_string))
        .map_or(DownloadRouteSource::Official, |host| match host.as_str() {
            FETCH_BMCLAPI2_BANGBANG93_COM => DownloadRouteSource::Bmclapi,
            FETCH_MOD_MCIMIRROR_TOP => DownloadRouteSource::Mcim,
            FETCH_MAVEN_ALIYUN_COM => DownloadRouteSource::Aliyun,
            _ => DownloadRouteSource::Official,
        });
    let is_mirror = matches!(
        source,
        DownloadRouteSource::Bmclapi
            | DownloadRouteSource::Mcim
            | DownloadRouteSource::Aliyun
    );
    let route = route(
        url,
        source,
        is_mirror,
        !matches!(resource, ResourceClass::Metadata),
    );
    #[cfg(test)]
    let route = {
        let mut route = route;
        if parsed_url
            .as_ref()
            .and_then(|url| url.host_str().and_then(|host| host.parse().ok()))
            .is_some_and(|address: std::net::IpAddr| address.is_loopback())
        {
            route.proxy = ProxyPolicy::Direct;
        }
        route
    };
    route
}

pub(super) fn is_official_route(route: &DownloadRoute) -> bool {
    !route.is_mirror && route.source == DownloadRouteSource::Official
}

pub(super) fn official_fallback_route(
    routes: &[DownloadRoute],
) -> Option<DownloadRoute> {
    routes
        .iter()
        .find(|candidate| is_official_route(candidate))
        .cloned()
}

fn url_with_base(original: &Url, base: &str, path: &str) -> Option<String> {
    let mut target = Url::parse(base).ok()?;
    target.set_path(path);
    target.set_query(original.query());
    target.set_fragment(None);
    Some(target.into())
}

fn custom_url_with_path(
    original: &Url,
    base: &str,
    path: &str,
) -> Option<String> {
    let mut target = Url::parse(base).ok()?;
    let base_path = target.path().trim_end_matches('/');
    target.set_path(&format!("{base_path}{path}"));
    target.set_query(original.query());
    target.set_fragment(None);
    Some(target.into())
}

pub(super) fn explicit_mirror_routes(
    url: &str,
    resource: ResourceClass,
) -> Vec<DownloadRoute> {
    let Ok(parsed) = Url::parse(url) else {
        return Vec::new();
    };
    if parsed.scheme() != HTTPS_SCHEME {
        return Vec::new();
    }

    let host = parsed.host_str().unwrap_or_default();
    let path = parsed.path();
    let supports_range = !matches!(resource, ResourceClass::Metadata);
    let mut routes = Vec::new();
    let push_mirror = |routes: &mut Vec<DownloadRoute>,
                       base: &str,
                       path: String,
                       source: DownloadRouteSource| {
        if let Some(url) = url_with_base(&parsed, base, &path) {
            routes.push(route(url, source, true, supports_range));
        }
    };

    match host {
        FETCH_RESOURCES_DOWNLOAD_MINECRAFT_NET => {
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                format!("/assets{path}"),
                DownloadRouteSource::Bmclapi,
            );
        }
        FETCH_LIBRARIES_MINECRAFT_NET => {
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                format!("/maven{path}"),
                DownloadRouteSource::Bmclapi,
            );
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                format!("/libraries{path}"),
                DownloadRouteSource::Bmclapi,
            );
        }
        // Maven Central hosts the bulk of loader-era libraries (Cleanroom,
        // LiteLoader, Babric, legacy Forge processors). Aliyun's public
        // repository mirrors it 1:1 and is reachable without a proxy, which
        // keeps a flaky proxy from stalling a whole install on one authority.
        FETCH_REPO_MAVEN_APACHE_ORG | FETCH_REPO1_MAVEN_ORG
            if path.starts_with(FETCH_MAVEN2) =>
        {
            push_mirror(
                &mut routes,
                ALIYUN_MAVEN_BASE_URL,
                format!(
                    "/repository/public{}",
                    path.trim_start_matches("/maven2")
                ),
                DownloadRouteSource::Aliyun,
            );
        }
        ROUTE_POLICY_MAVEN_MINECRAFTFORGE_NET
        | ROUTE_POLICY_MAVEN_FABRICMC_NET => {
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                format!("/maven{path}"),
                DownloadRouteSource::Bmclapi,
            );
        }
        FETCH_FILES_MINECRAFTFORGE_NET if path.starts_with(FETCH_MAVEN) => {
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                path.to_string(),
                DownloadRouteSource::Bmclapi,
            );
        }
        ROUTE_POLICY_MAVEN_NEOFORGED_NET
            if path.starts_with(FETCH_RELEASES) =>
        {
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                format!("/maven/{}", path.trim_start_matches("/releases/")),
                DownloadRouteSource::Bmclapi,
            );
        }
        FETCH_META_FABRICMC_NET => {
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                format!("/fabric-meta{path}"),
                DownloadRouteSource::Bmclapi,
            );
        }
        ROUTE_POLICY_PISTON_META_MOJANG_COM
        | ROUTE_POLICY_LAUNCHERMETA_MOJANG_COM
        | FETCH_LAUNCHER_MOJANG_COM
        | FETCH_PISTON_DATA_MOJANG_COM => push_mirror(
            &mut routes,
            BMCLAPI_BASE_URL,
            path.to_string(),
            DownloadRouteSource::Bmclapi,
        ),
        _ => {}
    }

    routes
}

fn custom_mirror_routes(
    url: &str,
    resource: ResourceClass,
    sources: &[CustomDownloadSource],
) -> Vec<DownloadRoute> {
    let Ok(parsed) = Url::parse(url) else {
        return Vec::new();
    };
    if parsed.scheme() != HTTPS_SCHEME {
        return Vec::new();
    }

    let host = parsed.host_str().unwrap_or_default();
    let path = parsed.path();
    let supports_range = !matches!(resource, ResourceClass::Metadata);
    let mut ordered = sources
        .iter()
        .filter(|source| source.enabled)
        .collect::<Vec<_>>();
    ordered.sort_by_key(|source| source.priority);

    let mut routes = Vec::new();
    for source in ordered {
        let paths: Vec<String> = match (source.kind, resource, host) {
            (
                CustomDownloadSourceKind::Minecraft,
                ResourceClass::MinecraftAsset,
                FETCH_RESOURCES_DOWNLOAD_MINECRAFT_NET,
            ) => vec![format!("/assets{path}")],
            (
                CustomDownloadSourceKind::Minecraft,
                ResourceClass::MinecraftLibrary,
                FETCH_LIBRARIES_MINECRAFT_NET,
            ) => vec![format!("/maven{path}"), format!("/libraries{path}")],
            (
                CustomDownloadSourceKind::Minecraft,
                ResourceClass::MinecraftLibrary,
                FETCH_REPO_MAVEN_APACHE_ORG | FETCH_REPO1_MAVEN_ORG,
            ) if path.starts_with(FETCH_MAVEN2) => vec![format!(
                "/repository/public{}",
                path.trim_start_matches("/maven2")
            )],
            (
                CustomDownloadSourceKind::Minecraft,
                ResourceClass::Loader,
                ROUTE_POLICY_MAVEN_MINECRAFTFORGE_NET
                | ROUTE_POLICY_MAVEN_FABRICMC_NET,
            ) => vec![format!("/maven{path}")],
            (
                CustomDownloadSourceKind::Minecraft,
                ResourceClass::Loader,
                FETCH_FILES_MINECRAFTFORGE_NET,
            ) if path.starts_with(FETCH_MAVEN) => vec![path.to_string()],
            (
                CustomDownloadSourceKind::Minecraft,
                ResourceClass::Loader,
                ROUTE_POLICY_MAVEN_NEOFORGED_NET,
            ) if path.starts_with(FETCH_RELEASES) => vec![format!(
                "/maven/{}",
                path.trim_start_matches("/releases/")
            )],
            (
                CustomDownloadSourceKind::Minecraft,
                ResourceClass::Loader,
                FETCH_META_FABRICMC_NET,
            ) => vec![format!("/fabric-meta{path}")],
            (
                CustomDownloadSourceKind::Minecraft,
                ResourceClass::Metadata | ResourceClass::Java,
                ROUTE_POLICY_PISTON_META_MOJANG_COM
                | ROUTE_POLICY_LAUNCHERMETA_MOJANG_COM
                | FETCH_LAUNCHER_MOJANG_COM
                | FETCH_PISTON_DATA_MOJANG_COM,
            ) => vec![path.to_string()],
            (
                CustomDownloadSourceKind::Modrinth,
                ResourceClass::Modrinth | ResourceClass::Modpack,
                MODRINTH_CDN_HOST,
            ) => vec![path.to_string()],
            (
                CustomDownloadSourceKind::Curseforge,
                ResourceClass::CurseForge,
                CURSEFORGE_EDGE_HOST
                | RETIRED_SOURCES_MEDIA_FORGECDN_NET
                | FETCH_MEDIAFILEZ_FORGECDN_NET,
            ) => vec![path.to_string()],
            _ => Vec::new(),
        };

        for path in paths {
            if let Some(target) =
                custom_url_with_path(&parsed, &source.base_url, &path)
            {
                routes.push(route(
                    target,
                    DownloadRouteSource::Custom,
                    true,
                    supports_range,
                ));
            }
        }
    }
    routes
}

fn explicit_mirror_routes_with_custom(
    url: &str,
    resource: ResourceClass,
    sources: &[CustomDownloadSource],
) -> Vec<DownloadRoute> {
    let mut custom = custom_mirror_routes(url, resource, sources);
    custom.extend(explicit_mirror_routes(url, resource));
    custom
}

pub(crate) fn route_host(route: &DownloadRoute) -> Option<String> {
    Url::parse(&route.url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
}

pub(crate) fn is_official_modrinth_download_url(url: &str) -> bool {
    route_policy::is_official_modrinth_download_url(url)
}

fn is_official_version_manifest_url(url: &str) -> bool {
    route_policy::is_official_version_manifest_url(url)
}

pub(super) fn order_auto_routes(
    routes: &mut [DownloadRoute],
    resource: ResourceClass,
) {
    let health = ROUTE_HEALTH.lock().clone();
    routes.sort_by(|left, right| {
        let route_health = |route: &DownloadRoute| {
            let Some(key) = route_health_key(route, resource) else {
                return RouteHealth::default();
            };
            health
                .get(&key)
                .cloned()
                .unwrap_or_else(|| persisted_route_health(&key, route.proxy))
        };
        let left_health = route_health(left);
        let right_health = route_health(right);
        let now = Instant::now();
        let left_cooling =
            left_health.cooldown_until.is_some_and(|until| until > now);
        let right_cooling =
            right_health.cooldown_until.is_some_and(|until| until > now);
        left_cooling
            .cmp(&right_cooling)
            .then_with(|| {
                left_health
                    .consecutive_failures
                    .cmp(&right_health.consecutive_failures)
            })
            .then_with(|| {
                // Automatic content downloads start from the supplied
                // official URL. Health can still move a failing official
                // route behind a mirror, but connectivity alone is not a
                // reason to call a mirror faster.
                (!is_official_route(left)).cmp(&(!is_official_route(right)))
            })
            .then_with(|| {
                (right_health.success_samples > 0)
                    .cmp(&(left_health.success_samples > 0))
            })
            .then_with(|| {
                let ordering = if matches!(resource, ResourceClass::Metadata) {
                    left_health.ttfb_ms.partial_cmp(&right_health.ttfb_ms)
                } else {
                    right_health
                        .throughput_bps
                        .partial_cmp(&left_health.throughput_bps)
                };
                ordering.unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.is_mirror.cmp(&right.is_mirror))
    });
}

pub fn resolve_download_routes_for(
    url: &str,
    resource: ResourceClass,
    mode: crate::state::DownloadSourceMode,
) -> Vec<DownloadRoute> {
    let custom_sources = crate::State::get_if_initialized()
        .map(|state| state.custom_download_sources())
        .unwrap_or_default();
    resolve_download_routes_for_with_sources(
        url,
        resource,
        mode,
        &custom_sources,
    )
}

pub fn resolve_download_routes_for_with_sources(
    url: &str,
    resource: ResourceClass,
    mode: crate::state::DownloadSourceMode,
    custom_sources: &[CustomDownloadSource],
) -> Vec<DownloadRoute> {
    let Ok(url) = retired_sources::normalize(url) else {
        return Vec::new();
    };
    let url = url.into_owned();
    let official = official_route(&url, resource);
    let mut routes =
        explicit_mirror_routes_with_custom(&url, resource, custom_sources);
    routes.push(official);
    // Provider APIs carry credentials and must not be sent to mirrors.
    let official_only_api = is_modrinth_api_url(&url)
        || Url::parse(&url).ok().is_some_and(|parsed| {
            parsed.host_str() == Some(CURSEFORGE_API_HOST)
        });
    if official_only_api {
        routes.retain(|route| !route.is_mirror);
    }
    let mode = if official_only_api {
        crate::state::DownloadSourceMode::OfficialOnly
    } else {
        mode
    };
    match mode {
        crate::state::DownloadSourceMode::Auto
            if is_official_version_manifest_url(&url) =>
        {
            routes.sort_by_key(|route| route.is_mirror)
        }
        crate::state::DownloadSourceMode::Auto => {
            order_auto_routes(&mut routes, resource)
        }
        crate::state::DownloadSourceMode::OfficialOnly => {
            routes.retain(|route| !route.is_mirror);
        }
        crate::state::DownloadSourceMode::OfficialPreferred => {
            routes.sort_by_key(|route| route.is_mirror);
        }
        crate::state::DownloadSourceMode::MirrorPreferred => {
            routes.sort_by_key(|route| !route.is_mirror);
        }
    }
    routes
}

pub(super) fn source_mode_for_resource(
    resource: ResourceClass,
) -> crate::state::DownloadSourceMode {
    let Some(state) = crate::State::get_if_initialized() else {
        return crate::state::DownloadSourceMode::OfficialOnly;
    };

    match resource {
        ResourceClass::Metadata => state.minecraft_metadata_source(),
        ResourceClass::MinecraftAsset
        | ResourceClass::MinecraftLibrary
        | ResourceClass::Loader
        | ResourceClass::Java => state.minecraft_file_source(),
        ResourceClass::Modrinth | ResourceClass::Modpack => {
            state.modrinth_source()
        }
        ResourceClass::CurseForge => state.curseforge_source(),
        ResourceClass::Other => crate::state::DownloadSourceMode::OfficialOnly,
    }
}

pub(super) fn infer_resource_class(url: &str) -> ResourceClass {
    let Ok(parsed) = Url::parse(url) else {
        return ResourceClass::Other;
    };
    let host = parsed.host_str().unwrap_or_default();
    match host {
        FETCH_RESOURCES_DOWNLOAD_MINECRAFT_NET => ResourceClass::MinecraftAsset,
        FETCH_LIBRARIES_MINECRAFT_NET => ResourceClass::MinecraftLibrary,
        ROUTE_POLICY_MAVEN_MINECRAFTFORGE_NET
        | FETCH_FILES_MINECRAFTFORGE_NET
        | ROUTE_POLICY_MAVEN_FABRICMC_NET
        | ROUTE_POLICY_MAVEN_NEOFORGED_NET
        | FETCH_META_FABRICMC_NET => ResourceClass::Loader,
        FETCH_REPO1_MAVEN_ORG | FETCH_REPO_MAVEN_APACHE_ORG => {
            ResourceClass::MinecraftLibrary
        }
        ROUTE_POLICY_PISTON_META_MOJANG_COM
            if parsed.path().contains(FETCH_JAVA_RUNTIME) =>
        {
            ResourceClass::Java
        }
        FETCH_PISTON_DATA_MOJANG_COM
            if parsed.path().contains(FETCH_JAVA_RUNTIME) =>
        {
            ResourceClass::Java
        }
        ROUTE_POLICY_PISTON_META_MOJANG_COM
        | ROUTE_POLICY_LAUNCHERMETA_MOJANG_COM => ResourceClass::Metadata,
        FETCH_LAUNCHER_MOJANG_COM | FETCH_PISTON_DATA_MOJANG_COM => {
            ResourceClass::MinecraftLibrary
        }
        MODRINTH_API_HOST | modrinth_cdn::HOST => ResourceClass::Modrinth,
        CURSEFORGE_API_HOST
        | CURSEFORGE_EDGE_HOST
        | RETIRED_SOURCES_MEDIA_FORGECDN_NET
        | FETCH_MEDIAFILEZ_FORGECDN_NET => ResourceClass::CurseForge,
        _ => ResourceClass::Other,
    }
}
