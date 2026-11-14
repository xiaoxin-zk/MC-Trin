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
    ROUTE_MIRROR_FAILURE_DEMOTION_THRESHOLD,
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
        // BMCLAPI preserves the path of launcher metadata (version manifests,
        // version JSON, Java runtime manifests) and of `piston-meta` packages.
        ROUTE_POLICY_PISTON_META_MOJANG_COM
        | ROUTE_POLICY_LAUNCHERMETA_MOJANG_COM => push_mirror(
            &mut routes,
            BMCLAPI_BASE_URL,
            path.to_string(),
            DownloadRouteSource::Bmclapi,
        ),
        // Only the object names BMCLAPI actually stores may be mapped onto its
        // `/v1/objects/<sha>/<name>` endpoint. The endpoint is content
        // addressed, but the mirror only knows the objects it synchronised from
        // the version manifest (client/server jars and their mappings); every
        // other object name — including the log4j `client-1.12.xml` config — is
        // a guaranteed 404 there and must not produce a mirror route.
        FETCH_PISTON_DATA_MOJANG_COM | FETCH_LAUNCHER_MOJANG_COM
            if bmclapi_serves_mojang_object(path) =>
        {
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                path.to_string(),
                DownloadRouteSource::Bmclapi,
            );
        }
        _ => {}
    }

    routes
}

/// Object paths under `piston-data`/`launcher.mojang.com` that BMCLAPI mirrors.
///
/// BMCLAPI's object endpoint is content addressed, but it only holds the
/// objects it indexed from the version and Java-runtime manifests: client and
/// server jars, their ProGuard mappings, and the runtime's own files (all
/// verified serving `302` on the real service). It does *not* hold the log4j
/// configuration from `logging.client.file`: `GET
/// /v1/objects/<sha1>/client-1.12.xml` answers `404` there while the very same
/// object exists on Mojang's host. Emitting a route for it therefore only
/// manufactures a mirror failure, and that failure used to be recorded against
/// the whole mirror authority, demoting it for every later download in the
/// session.
///
/// The rule is deliberately narrow: only the endpoint's real shape is accepted
/// (a flat file name under a 40-hex-digit hash), and the `.xml` log-config
/// family — the one object shape proven missing — is refused. Anything not
/// recognised simply keeps its official URL, which is the same behaviour as any
/// other host without a mirror.
fn bmclapi_serves_mojang_object(path: &str) -> bool {
    const V1_OBJECTS_PREFIX: &str = "/v1/objects/";
    const SHA1_HEX_LEN: usize = 40;
    const LOG_CONFIG_SUFFIX: &str = ".xml";
    let Some((hash, name)) = path
        .strip_prefix(V1_OBJECTS_PREFIX)
        .and_then(|rest| rest.split_once('/'))
    else {
        return false;
    };
    hash.len() == SHA1_HEX_LEN
        && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        && !name.is_empty()
        && !name.contains('/')
        && !name.ends_with(LOG_CONFIG_SUFFIX)
}

/// Mirrors supplied by the user for one resource class.
///
/// Only Minecraft ships a built-in mirror set in this build (BMCLAPI for
/// assets, libraries and loaders; Aliyun for Maven Central). There is **no**
/// built-in mirror for `cdn.modrinth.com`, `edge.forgecdn.net` or
/// `mediafilez.forgecdn.net`: the `modrinth_source` / `curseforge_source`
/// preferences can only order a mirror the user added to
/// `custom_download_sources`, and with an empty list `mirror_preferred` and
/// `official_only` produce exactly the same routes. The settings UI says so
/// next to those selectors instead of implying a mirror that does not exist.
///
/// MCIM is not emitted by any builder either: a route only becomes
/// [`DownloadRouteSource::Mcim`] when the supplied URL is already an MCIM URL
/// (for example a saved pack that pins one), which is why the MCIM pin exists
/// but no built-in mirror claims to serve those CDNs.
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

/// MCIM asks to be tried after the official hosts on every request.
///
/// Every caller that can produce a mixed route list runs this last, so no
/// mode-specific sort can hand MCIM the lead: a source preference is not a
/// reason to send MCIM a request it explicitly refuses to serve first.
///
/// Note that no builder emits an MCIM route: it is only ever produced by
/// [`official_route`] when the supplied URL already points at MCIM (a saved pack
/// that pins `mod.mcimirror.top`). `pin_mcim_last` therefore only protects such
/// restored URLs, and the absence of a builder is why the setting cannot be
/// presented as a built-in mirror.
fn pin_mcim_last(routes: &mut [DownloadRoute]) {
    routes.sort_by_key(|route| route.source == DownloadRouteSource::Mcim);
}

pub(super) fn order_auto_routes(
    routes: &mut [DownloadRoute],
    resource: ResourceClass,
    prefer_mirrors: bool,
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
        // Without a mirror preference any failure is enough to try the other
        // origin first. With one, a lone failure must not outrank the user's
        // choice: a single 404 describes one object, not an unreachable
        // mirror, and letting it pin the official host for the whole session is
        // exactly the failure mode this ordering has to avoid. Only a streak
        // that reaches the demotion threshold is evidence that the mirror
        // itself is down; it then falls behind an official route, while a
        // genuinely dead official route still loses to a healthier mirror.
        let failure_rank = |health: &RouteHealth| {
            let failures = health.consecutive_failures;
            if prefer_mirrors {
                let demoted =
                    failures >= ROUTE_MIRROR_FAILURE_DEMOTION_THRESHOLD;
                (demoted, if demoted { failures } else { 0 })
            } else {
                (failures > 0, failures)
            }
        };
        let left_failures = failure_rank(&left_health);
        let right_failures = failure_rank(&right_health);
        (left.source == DownloadRouteSource::Mcim)
            .cmp(&(right.source == DownloadRouteSource::Mcim))
            .then_with(|| left_cooling.cmp(&right_cooling))
            .then_with(|| left_failures.cmp(&right_failures))
            .then_with(|| {
                // Healthy routes are already resolved by the keys above: the
                // MCIM pin, cooldown, mirror demotion and consecutive-failure
                // comparisons run first, so this tie-break only orders routes
                // that are equally reachable. A Chinese-locale preference leads
                // with the mirror and keeps the official route as the next
                // fallback; every other locale starts from the supplied
                // official URL. MCIM is pinned by the first key, so `is_mirror`
                // cannot promote it.
                if prefer_mirrors {
                    right.is_mirror.cmp(&left.is_mirror)
                } else {
                    let left_non_official = !is_official_route(left);
                    let right_non_official = !is_official_route(right);
                    left_non_official.cmp(&right_non_official)
                }
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
    let prefer_mirrors = crate::State::get_if_initialized()
        .map(|state| state.auto_prefers_mirror())
        .unwrap_or(false);
    resolve_download_routes_for_with_sources(
        url,
        resource,
        mode,
        &custom_sources,
        prefer_mirrors,
    )
}

/// Applies the mode's ordering and then the MCIM pin.
///
/// Split from route construction so the ordering contract is testable with a
/// mixed list: no route builder in this build emits an MCIM mirror beside an
/// official URL, and a test that relied on one would pass vacuously.
///
/// `prefer_mirrors` is the stored locale preference and only affects `Auto`:
/// every explicit mode states its own source order and must ignore it.
///
/// The explicit modes are honest about what exists in this build: for
/// Minecraft there are built-in mirrors (BMCLAPI, Aliyun) so
/// `MirrorPreferred` really leads with one; for Modrinth and CurseForge there
/// are none, so `MirrorPreferred` only reorders user-supplied
/// `custom_download_sources` and otherwise resolves to the same single official
/// route as `OfficialOnly`. That equivalence is documented in the settings copy
/// rather than hidden behind a switch that appears to do more than it can.
pub(super) fn order_routes_for_mode(
    routes: &mut Vec<DownloadRoute>,
    resource: ResourceClass,
    mode: crate::state::DownloadSourceMode,
    url: &str,
    prefer_mirrors: bool,
) {
    match mode {
        crate::state::DownloadSourceMode::Auto
            if is_official_version_manifest_url(url) =>
        {
            routes.sort_by_key(|route| route.is_mirror)
        }
        crate::state::DownloadSourceMode::Auto => {
            order_auto_routes(routes, resource, prefer_mirrors)
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
    // MCIM asks to be a fallback on every request, and a source preference is
    // not a reason to hand it a file request it explicitly refuses to lead.
    // This runs after every mode so no path can put it first.
    pin_mcim_last(routes);
}

pub fn resolve_download_routes_for_with_sources(
    url: &str,
    resource: ResourceClass,
    mode: crate::state::DownloadSourceMode,
    custom_sources: &[CustomDownloadSource],
    prefer_mirrors: bool,
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
    order_routes_for_mode(&mut routes, resource, mode, &url, prefer_mirrors);
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
