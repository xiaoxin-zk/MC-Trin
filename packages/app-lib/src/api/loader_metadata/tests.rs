use super::*;
use std::io::Write;
use zip::write::SimpleFileOptions;

#[test]
fn forge_fallback_requires_exact_game_and_loader_version() {
    let manifest = forge_manifest(
        maven_metadata(
            "",
            &["1.20.1-47.4.28", "1.20.1-47.4.22", "1.21.1-52.1.0"],
        ),
        ForgePromotions::default(),
    );
    assert_eq!(
        find_exact_forge_version(manifest.clone(), "1.20.1", "47.4.28")
            .unwrap()
            .id,
        "47.4.28"
    );
    assert!(
        find_exact_forge_version(manifest.clone(), "1.21.1", "47.4.28")
            .is_none()
    );
    assert!(find_exact_forge_version(manifest, "1.20.1", "47.4.29").is_none());
}

fn maven_metadata(release: &str, versions: &[&str]) -> MavenMetadata {
    MavenMetadata {
        versioning: MavenVersioning {
            release: release.to_string(),
            versions: MavenVersions {
                values: versions
                    .iter()
                    .map(|version| version.to_string())
                    .collect(),
            },
        },
    }
}

#[tokio::test]
async fn forced_forge_refresh_replaces_fresh_memory_catalog() {
    let cache = LoaderManifestCache::default();
    let old = || async {
        Ok(CachedLoaderManifest {
            loader: "forge".into(),
            manifest: forge_manifest(
                maven_metadata("", &["1.20.1-47.4.22"]),
                ForgePromotions::default(),
            ),
        })
    };
    cache
        .get_or_fetch("forge", Some("1.20.1"), |_| old())
        .await
        .unwrap();
    let fresh = cache
        .get_or_fetch_with_refresh(
            "forge",
            Some("1.20.1"),
            true,
            |scope| async move {
                assert_eq!(scope, None);
                Ok(CachedLoaderManifest {
                    loader: "forge".into(),
                    manifest: forge_manifest(
                        maven_metadata(
                            "",
                            &["1.20.1-47.4.28", "1.21.1-52.1.0"],
                        ),
                        ForgePromotions::default(),
                    ),
                })
            },
        )
        .await
        .unwrap();
    assert_eq!(forge_loaders(&fresh.manifest, "1.20.1")[0].id, "47.4.28");
    let other = cache
        .get_or_fetch("forge", Some("1.21.1"), |_| async {
            panic!("refreshed full catalog should serve other game versions")
        })
        .await
        .unwrap();
    assert_eq!(forge_loaders(&other.manifest, "1.21.1")[0].id, "52.1.0");
}

#[tokio::test]
async fn forced_metadata_refresh_reports_failure_without_destroying_cache() {
    let cache = MetadataCache::default();
    cache.get_or_fetch(|| async { Ok(22) }).await.unwrap();
    let failed = cache
        .get_or_fetch_with_refresh(true, || async {
            Err(crate::ErrorKind::OtherError("refresh failed".into())
                .as_error())
        })
        .await;
    assert!(failed.unwrap_err().to_string().contains("refresh failed"));
    assert_eq!(
        cache
            .get_or_fetch(|| async { panic!("keep good cache") })
            .await
            .unwrap(),
        22
    );
    let retry = cache
        .get_or_fetch_with_refresh(true, || async {
            panic!("failure backoff should prevent a retry storm")
        })
        .await;
    assert!(retry.is_err());
}

#[tokio::test]
async fn simultaneous_forced_refreshes_share_the_inflight_request() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let cache = MetadataCache::default();
    cache.get_or_fetch(|| async { Ok(22) }).await.unwrap();
    let calls = AtomicUsize::new(0);
    let fetch = || async {
        calls.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        Ok(28)
    };
    let (first, second) = tokio::join!(
        cache.get_or_fetch_with_refresh(true, fetch),
        cache.get_or_fetch_with_refresh(true, fetch),
    );
    assert_eq!(first.unwrap(), 28);
    assert_eq!(second.unwrap(), 28);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn database_bypass_propagates_through_loader_memory_cache() {
    let entry = LOADER_MANIFESTS
        .entry(daedalus::modded::loader_manifest_metadata("forge").cache_key)
        .await;
    *entry.value.lock().await = Some((
        Instant::now(),
        CachedLoaderManifest {
            loader: "forge".into(),
            manifest: forge_manifest(
                maven_metadata("", &["1.20.1-47.4.22"]),
                ForgePromotions::default(),
            ),
        },
    ));
    *entry.failure.lock().await = Some((
        Instant::now(),
        Arc::new(crate::ErrorKind::OtherError(
            "forced refresh reached memory cache".into(),
        )),
    ));
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let semaphore = FetchSemaphore(tokio::sync::Semaphore::new(0));
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        crate::state::CachedEntry::get_loader_manifest(
            &daedalus::modded::loader_manifest_metadata_for_game(
                "forge", "1.20.1",
            )
            .cache_key,
            Some(crate::state::CacheBehaviour::Bypass),
            &pool,
            &semaphore,
        ),
    )
    .await;
    *entry.value.lock().await = None;
    *entry.failure.lock().await = None;
    pool.close().await;
    let error = result.unwrap().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("forced refresh reached memory cache")
    );
}

#[test]
fn scoped_catalog_prefers_explicit_game_over_earlier_placeholder() {
    let mut full = forge_manifest(
        maven_metadata("", &["1.20.1-47.4.28"]),
        ForgePromotions::default(),
    );
    let mut generic = full.game_versions[0].clone();
    generic.id = DUMMY_REPLACE_STRING.into();
    generic.loaders.clear();
    full.game_versions.insert(0, generic);
    let scoped = scope_manifest(full, "1.20.1");
    assert_eq!(forge_loaders(&scoped, "1.20.1")[0].id, "47.4.28");
}

#[tokio::test]
async fn catalog_loaders_share_downloads_without_mixing_game_versions() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let cache = LoaderManifestCache::default();
    for loader in [
        "forge",
        "legacy_fabric",
        "babric",
        "lite_loader",
        "cleanroom",
    ] {
        let calls = AtomicUsize::new(0);
        let fetch = |scope| {
            assert_eq!(scope, None);
            async {
                calls.fetch_add(1, Ordering::Relaxed);
                tokio::task::yield_now().await;
                Ok(CachedLoaderManifest {
                    loader: loader.into(),
                    manifest: forge_manifest(
                        maven_metadata("", &["1.20.1-47.1.0", "1.21.1-52.1.0"]),
                        ForgePromotions::default(),
                    ),
                })
            }
        };
        let (full, first, second) = tokio::join!(
            cache.get_or_fetch(loader, None, fetch),
            cache.get_or_fetch(loader, Some("1.20.1"), fetch),
            cache.get_or_fetch(loader, Some("1.21.1"), fetch),
        );
        assert_eq!(full.unwrap().manifest.game_versions.len(), 2);
        let first = first.unwrap().manifest;
        let second = second.unwrap().manifest;
        assert_eq!(first.game_versions.len(), 1);
        assert_eq!(first.game_versions[0].id, "1.20.1");
        assert_eq!(first.game_versions[0].loaders[0].id, "47.1.0");
        assert_eq!(second.game_versions[0].id, "1.21.1");
        assert_eq!(second.game_versions[0].loaders[0].id, "52.1.0");
        let unsupported = cache
            .get_or_fetch(loader, Some("1.2.3"), fetch)
            .await
            .unwrap();
        assert!(unsupported.manifest.game_versions[0].loaders.is_empty());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}

#[tokio::test]
async fn known_unsupported_versions_return_without_network_or_database_access()
{
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_lazy("sqlite::memory:")
        .unwrap();
    let semaphore = FetchSemaphore(tokio::sync::Semaphore::new(0));
    for (loader, game) in [
        ("babric", "1.21.1"),
        ("cleanroom", "1.20.1"),
        ("lite_loader", "1.6.4"),
    ] {
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            fetch_loader_manifest_official_first(
                loader,
                Some(game),
                "https://invalid.test/manifest.json",
                &semaphore,
                &pool,
                false,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.manifest.game_versions[0].id, game);
        assert!(result.manifest.game_versions[0].loaders.is_empty());
    }
}

#[tokio::test]
async fn scoped_loader_caches_preserve_compatibility_and_empty_results() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let cache = LoaderManifestCache::default();
    for loader in ["fabric", "quilt", "optifine", "neo"] {
        let calls = AtomicUsize::new(0);
        for (game, versions) in [
            ("1.21.1", vec![("compatible".into(), true)]),
            ("1.2.3", vec![]),
        ] {
            let fetch = |scope| {
                assert_eq!(scope, Some(game));
                calls.fetch_add(1, Ordering::Relaxed);
                let versions = versions.clone();
                async move {
                    Ok(CachedLoaderManifest {
                        loader: loader.into(),
                        manifest: meta_api_manifest_for_game(
                            game,
                            versions,
                            loader,
                            FABRIC_META_URL,
                            0,
                        ),
                    })
                }
            };
            let first =
                cache.get_or_fetch(loader, Some(game), fetch).await.unwrap();
            let repeated = cache
                .get_or_fetch(loader, Some(game), |_| async {
                    panic!("scoped cache should avoid repeated downloads")
                })
                .await
                .unwrap();
            assert_eq!(
                first.manifest.game_versions[0].loaders.len(),
                versions.len()
            );
            assert_eq!(repeated.manifest.game_versions[0].id, game);
        }
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }
}

#[tokio::test]
async fn manifest_cache_capacity_preserves_active_requests() {
    let cache = LoaderManifestCache::default();
    let active = cache.entry("active".into()).await;
    for index in 0..MAX_LOADER_METADATA_ENTRIES * 2 {
        cache.entry(format!("key-{index}")).await;
    }
    assert_eq!(
        cache.entries.lock().await.len(),
        MAX_LOADER_METADATA_ENTRIES
    );
    assert!(Arc::ptr_eq(&active, &cache.entry("active".into()).await));
}

#[tokio::test]
async fn metadata_cache_coalesces_failures_and_retries_after_cooldown() {
    let cache = MetadataCache::<MavenMetadata>::default();
    let failed = cache
        .get_or_fetch(|| async {
            Err(crate::ErrorKind::OtherError("offline".into()).as_error())
        })
        .await;
    assert!(failed.is_err());
    assert!(
        cache
            .get_or_fetch(|| async {
                panic!("failure cooldown must prevent another request")
            })
            .await
            .is_err()
    );
    cache.failure.lock().await.as_mut().unwrap().0 =
        Instant::now() - LOADER_METADATA_RETRY_DELAY;
    assert!(
        cache
            .get_or_fetch(|| async { Ok(maven_metadata("", &["21.1.200"])) })
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn forge_fetches_catalog_and_promotions_concurrently() {
    let barrier = tokio::sync::Barrier::new(2);
    let (metadata, promotions) = tokio::time::timeout(
        Duration::from_secs(1),
        fetch_forge_indexes(
            async {
                barrier.wait().await;
                Ok(maven_metadata("", &["1.20.1-47.1.0"]))
            },
            async {
                barrier.wait().await;
                Ok(ForgePromotions {
                    promos: HashMap::from([(
                        "1.20.1-recommended".into(),
                        "47.1.0".into(),
                    )]),
                })
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let manifest = forge_manifest(metadata, promotions);
    assert!(manifest.game_versions[0].loaders[0].stable);
}

#[tokio::test]
async fn forge_keeps_catalog_when_promotions_stall_or_fail() {
    let (metadata, promotions) = tokio::time::timeout(
        FORGE_PROMOTIONS_TIMEOUT + Duration::from_secs(1),
        fetch_forge_indexes(
            async { Ok(maven_metadata("", &["1.20.1-47.1.0"])) },
            std::future::pending(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(promotions.promos.is_empty());
    assert_eq!(
        forge_manifest(metadata, promotions).game_versions[0]
            .loaders
            .len(),
        1
    );
    assert!(
        fetch_forge_indexes(
            async { Ok(maven_metadata("", &["1.20.1-47.1.0"])) },
            async {
                Err(crate::ErrorKind::OtherError("offline".into()).as_error())
            },
        )
        .await
        .is_ok()
    );
    assert!(
        fetch_forge_indexes(
            async {
                Err(crate::ErrorKind::OtherError("invalid XML".into())
                    .as_error())
            },
            async { Ok(ForgePromotions::default()) },
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn neoforge_scoped_requests_skip_the_unrelated_index() {
    for game_version in ["1.21.1", "26.3", "1.0.25w14craftmine"] {
        let (current, legacy) = fetch_neoforge_indexes(
            Some(game_version),
            || async { Ok(maven_metadata("21.1.200", &["21.1.200"])) },
            || async {
                panic!("modern versions must not fetch the legacy index")
            },
        )
        .await
        .unwrap();
        assert_eq!(current.versioning.versions.values.len(), 1);
        assert!(legacy.versioning.versions.values.is_empty());
    }
    let (current, legacy) = fetch_neoforge_indexes(
        Some("1.20.1"),
        || async { panic!("1.20.1 must not fetch the modern index") },
        || async { Ok(maven_metadata("", &["1.20.1-47.1.106"])) },
    )
    .await
    .unwrap();
    assert!(current.versioning.versions.values.is_empty());
    let manifest = neoforge_manifest(current, legacy);
    assert_eq!(manifest.game_versions[0].id, "1.20.1");
    assert_eq!(manifest.game_versions[0].loaders[0].id, "47.1.106");
}

#[tokio::test]
async fn neoforge_full_catalog_and_scoped_requests_share_index_downloads() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let current = MetadataCache::default();
    let legacy = MetadataCache::default();
    let current_calls = AtomicUsize::new(0);
    let legacy_calls = AtomicUsize::new(0);
    let fetch_current = || {
        current.get_or_fetch(|| async {
            current_calls.fetch_add(1, Ordering::Relaxed);
            tokio::task::yield_now().await;
            Ok(maven_metadata("21.1.200", &["21.1.200", "26.3.0.1"]))
        })
    };
    let fetch_legacy = || {
        legacy.get_or_fetch(|| async {
            legacy_calls.fetch_add(1, Ordering::Relaxed);
            tokio::task::yield_now().await;
            Ok(maven_metadata("", &["1.20.1-47.1.106"]))
        })
    };
    let (full, modern, old) = tokio::join!(
        fetch_neoforge_indexes(None, fetch_current, fetch_legacy),
        fetch_neoforge_indexes(Some("1.21.1"), fetch_current, fetch_legacy),
        fetch_neoforge_indexes(Some("1.20.1"), fetch_current, fetch_legacy),
    );
    let (current_metadata, legacy_metadata) = full.unwrap();
    assert_eq!(
        neoforge_manifest(current_metadata, legacy_metadata)
            .game_versions
            .len(),
        3
    );
    assert_eq!(modern.unwrap().0.versioning.versions.values.len(), 2);
    assert_eq!(old.unwrap().1.versioning.versions.values.len(), 1);
    fetch_neoforge_indexes(Some("26.3"), fetch_current, fetch_legacy)
        .await
        .unwrap();
    assert_eq!(current_calls.load(Ordering::Relaxed), 1);
    assert_eq!(legacy_calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn neoforge_index_refreshes_after_expiry_and_preserves_data_on_failure() {
    let cache = MetadataCache::default();
    *cache.value.lock().await = Some((
        Instant::now() - LOADER_METADATA_TTL,
        maven_metadata("21.1.200", &["21.1.200"]),
    ));
    let updated = cache
        .get_or_fetch(|| async {
            Ok(maven_metadata("21.1.201", &["21.1.200", "21.1.201"]))
        })
        .await
        .unwrap();
    assert_eq!(updated.versioning.release, "21.1.201");
    let cached = cache
        .get_or_fetch(|| async { panic!("fresh cache must avoid fetching") })
        .await
        .unwrap();
    assert_eq!(cached.versioning.release, "21.1.201");
    cache.value.lock().await.as_mut().unwrap().0 =
        Instant::now() - LOADER_METADATA_TTL;
    let fail = || async {
        Err(crate::ErrorKind::OtherError("offline".into()).as_error())
    };
    let stale = cache.get_or_fetch(fail).await.unwrap();
    assert_eq!(stale.versioning.release, "21.1.201");
    assert!(
        MetadataCache::<MavenMetadata>::default()
            .get_or_fetch(fail)
            .await
            .is_err()
    );
}

#[test]
fn maven_index_validation_rejects_invalid_and_empty_responses() {
    for xml in [
        "not XML",
        "<html><body>upstream unavailable</body></html>",
        "<metadata><versioning><versions/></versioning></metadata>",
    ] {
        assert!(
            parse_maven_metadata(
                "https://example.test/index.xml",
                xml.as_bytes()
            )
            .is_err()
        );
    }
    let metadata = parse_maven_metadata(
            "https://example.test/index.xml",
            b"<metadata><versioning><versions><version>21.1.200</version></versions></versioning></metadata>",
        ).unwrap();
    assert_eq!(metadata.versioning.versions.values, ["21.1.200"]);
}

fn forge_loaders<'a>(
    manifest: &'a Manifest,
    game_version: &str,
) -> &'a [LoaderVersion] {
    &manifest
        .game_versions
        .iter()
        .find(|version| version.id == game_version)
        .unwrap_or_else(|| panic!("missing Forge support for {game_version}"))
        .loaders
}

#[test]
fn forge_metadata_groups_versions_and_uses_installer_profiles() {
    let manifest = forge_manifest(
        maven_metadata("1.21.5-55.1.13", &["1.21.5-55.1.9", "1.21.5-55.1.13"]),
        ForgePromotions {
            promos: HashMap::from([
                ("1.21.5-latest".to_string(), "55.1.13".to_string()),
                ("1.21.5-recommended".to_string(), "55.1.9".to_string()),
            ]),
        },
    );
    let loaders = &manifest.game_versions[0].loaders;
    assert_eq!(loaders[0].id, "55.1.13");
    assert_eq!(loaders[0].profile_source, LoaderProfileSource::Installer);
    assert!(!loaders[0].stable);
    assert!(loaders[1].stable);
    assert!(
        loaders[0]
            .url
            .ends_with("/1.21.5-55.1.13/forge-1.21.5-55.1.13-installer.jar")
    );
}

#[test]
fn forge_maven_metadata_parses_modern_version_coordinates() {
    let metadata: MavenMetadata = quick_xml::de::from_str(
        r#"<metadata>
                <versioning>
                    <release>26.2-65.1.0</release>
                    <versions>
                        <version>1.18.2-40.3.12</version>
                        <version>1.19-41.1.0</version>
                        <version>1.20.1-47.4.22</version>
                        <version>1.21.1-52.1.16</version>
                        <version>26.1-62.0.9</version>
                        <version>26.2-65.1.0</version>
                    </versions>
                </versioning>
            </metadata>"#,
    )
    .unwrap();
    let manifest = forge_manifest(metadata, ForgePromotions::default());

    for (game_version, loader_prefix) in [
        ("1.18.2", "40."),
        ("1.19", "41."),
        ("1.20.1", "47."),
        ("1.21.1", "52."),
        ("26.1", "62."),
        ("26.2", "65."),
    ] {
        assert!(
            forge_loaders(&manifest, game_version)
                .iter()
                .any(|loader| loader.id.starts_with(loader_prefix)),
            "missing Forge {loader_prefix} for Minecraft {game_version}"
        );
    }
}

#[test]
fn forge_preserves_complete_maven_catalog_and_maps_promotions() {
    let metadata: MavenMetadata = quick_xml::de::from_str(
        r#"<metadata>
                <versioning>
                    <release>26.2-65.1.1</release>
                    <versions>
                        <version>26.2-65.1.1</version>
                        <version>26.2-65.1.0</version>
                        <version>26.2-65.0.9</version>
                        <version>26.2-65.0.8</version>
                        <version>26.2-65.0.1</version>
                        <version>26.2-65.0.0</version>
                        <version>26.1-62.0.9</version>
                    </versions>
                </versioning>
            </metadata>"#,
    )
    .unwrap();
    let manifest = forge_manifest(
        metadata,
        ForgePromotions {
            promos: HashMap::from([
                ("26.2-latest".to_string(), "65.1.1".to_string()),
                ("26.2-recommended".to_string(), "65.1.0".to_string()),
            ]),
        },
    );
    let loaders = forge_loaders(&manifest, "26.2");

    assert_eq!(
        loaders
            .iter()
            .map(|loader| loader.id.as_str())
            .collect::<Vec<_>>(),
        ["65.1.1", "65.1.0", "65.0.9", "65.0.8", "65.0.1", "65.0.0"]
    );
    assert!(!loaders[0].stable);
    assert!(loaders[1].stable);
    assert!(loaders[2..].iter().all(|loader| !loader.stable));
    assert!(
        forge_loaders(&manifest, "26.1")
            .iter()
            .all(|loader| !loader.id.starts_with("65."))
    );
}

#[test]
fn forge_promotions_do_not_create_catalog_entries() {
    let manifest = forge_manifest(
        maven_metadata("26.2-65.1.0", &["26.2-65.1.0"]),
        ForgePromotions {
            promos: HashMap::from([(
                "26.2-latest".to_string(),
                "65.1.1".to_string(),
            )]),
        },
    );

    assert_eq!(forge_loaders(&manifest, "26.2")[0].id, "65.1.0");
}

#[test]
fn forge_scoped_metadata_keeps_empty_and_supported_versions_isolated() {
    let full = forge_manifest(
        maven_metadata(
            "1.20.1-47.4.22",
            &["1.20.1-47.4.21", "1.20.1-47.4.22", "1.21.5-55.1.13"],
        ),
        ForgePromotions::default(),
    );

    let forge_262 = scope_manifest(full.clone(), "26.2");
    let forge_1201 = scope_manifest(full.clone(), "1.20.1");
    let forge_262_again = scope_manifest(full, "26.2");

    assert!(forge_262.game_versions[0].loaders.is_empty());
    assert_eq!(
        forge_1201.game_versions[0]
            .loaders
            .iter()
            .map(|loader| loader.id.as_str())
            .collect::<Vec<_>>(),
        ["47.4.22", "47.4.21"]
    );
    assert!(forge_262_again.game_versions[0].loaders.is_empty());
    assert!(validate_scoped_manifest(forge_262, "26.2").is_ok());
}

#[test]
fn forge_scoped_metadata_preserves_complete_catalog_across_switches() {
    let full = forge_manifest(
        maven_metadata(
            "26.2-65.1.1",
            &[
                "26.2-65.1.1",
                "26.2-65.1.0",
                "26.2-65.0.9",
                "1.20.1-47.4.22",
                "1.20.1-47.4.21",
            ],
        ),
        ForgePromotions::default(),
    );

    let forge_262 = scope_manifest(full.clone(), "26.2");
    let forge_1201 = scope_manifest(full.clone(), "1.20.1");
    let forge_262_again = scope_manifest(full, "26.2");
    let ids = |manifest: &Manifest| {
        manifest.game_versions[0]
            .loaders
            .iter()
            .map(|loader| loader.id.clone())
            .collect::<Vec<_>>()
    };

    assert_eq!(ids(&forge_262), ["65.1.1", "65.1.0", "65.0.9"]);
    assert_eq!(ids(&forge_1201), ["47.4.22", "47.4.21"]);
    assert_eq!(ids(&forge_262_again), ids(&forge_262));
}

#[test]
fn meta_api_manifest_uses_official_profiles_and_fallbacks() {
    let manifest = meta_api_manifest(
        vec![MetaGameVersion {
            version: "1.21.1".to_string(),
            stable: true,
        }],
        vec![("0.16.9".to_string(), false), ("0.16.14".to_string(), true)],
        "fabric",
        FABRIC_META_URL,
        0,
    );
    let version = &manifest.game_versions[0];
    let loaders = &manifest.version_groups[0].loaders;
    assert_eq!(version.version_group.as_deref(), Some("fabric-official"));
    assert_eq!(loaders[0].id, "0.16.14");
    assert!(
        loaders[0]
            .url
            .contains("${modrinth.gameVersion}/0.16.14/profile/json")
    );
    assert!(
        loaders[0]
            .fallback_url
            .as_ref()
            .unwrap()
            .ends_with("fabric/v0/versions/0.16.14.json")
    );
}

#[test]
fn scoped_meta_manifest_binds_profile_and_versions_to_requested_game() {
    assert_eq!(
        meta_loader_versions_url(FABRIC_META_URL, "1.20.1"),
        "https://meta.fabricmc.net/v2/versions/loader/1.20.1"
    );
    assert_eq!(
        meta_loader_versions_url(QUILT_META_URL, "26.2"),
        "https://meta.quiltmc.org/v3/versions/loader/26.2"
    );
    let manifest = meta_api_manifest_for_game(
        "1.20.1",
        vec![("0.16.9".to_string(), false), ("0.16.14".to_string(), true)],
        "fabric",
        FABRIC_META_URL,
        0,
    );

    let game = &manifest.game_versions[0];
    assert_eq!(game.id, "1.20.1");
    assert_eq!(game.loaders[0].id, "0.16.14");
    assert!(
        game.loaders[0]
            .url
            .contains("loader/1.20.1/0.16.14/profile/json")
    );
    assert!(!game.loaders[0].url.contains(DUMMY_REPLACE_STRING));
}

#[test]
fn neoforge_scoped_metadata_does_not_mix_build_series() {
    let full = neoforge_manifest(
        maven_metadata("21.2.10", &["21.1.200", "21.1.201", "21.2.10"]),
        maven_metadata("", &[]),
    );

    let mc_1211 = scope_manifest(full.clone(), "1.21.1");
    let mc_1212 = scope_manifest(full.clone(), "1.21.2");
    let mc_1211_again = scope_manifest(full, "1.21.1");

    assert!(
        mc_1211.game_versions[0]
            .loaders
            .iter()
            .all(|loader| loader.id.starts_with("21.1."))
    );
    assert!(
        mc_1212.game_versions[0]
            .loaders
            .iter()
            .all(|loader| loader.id.starts_with("21.2."))
    );
    assert_eq!(
        mc_1211.game_versions[0]
            .loaders
            .iter()
            .map(|loader| loader.id.as_str())
            .collect::<Vec<_>>(),
        mc_1211_again.game_versions[0]
            .loaders
            .iter()
            .map(|loader| loader.id.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn neoforge_versions_map_to_minecraft_versions() {
    assert_eq!(neoforge_game_version("20.2.93"), Some("1.20.2".into()));
    assert_eq!(neoforge_game_version("21.0.167"), Some("1.21".into()));
    assert_eq!(neoforge_game_version("21.11.45"), Some("1.21.11".into()));
    assert_eq!(neoforge_game_version("26.1.0.19-beta"), Some("26.1".into()));
    assert_eq!(neoforge_game_version("26.1.2.95"), Some("26.1.2".into()));
    assert_eq!(
        neoforge_game_version("0.25w14craftmine.5-beta"),
        Some("1.0.25w14craftmine".into())
    );
}

#[test]
fn installer_parser_merges_profile_and_embedded_artifacts() {
    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default();
    archive.start_file("version.json", options).unwrap();
    archive
            .write_all(
                br#"{
                    "id":"1.21.5-forge-55.1.13",
                    "inheritsFrom":"1.21.5",
                    "releaseTime":"2026-08-15T20:37:45+00:00",
                    "time":"2026-08-15T20:37:45+00:00",
                    "mainClass":"example.Main",
                    "libraries":[{
                        "name":"example:generated:1.0",
                        "downloads":{"artifact":{"path":"example/generated/1.0/generated-1.0.jar","sha1":"abc","size":1,"url":""}}
                    }],
                    "type":"release"
                }"#,
            )
            .unwrap();
    archive.start_file("install_profile.json", options).unwrap();
    archive
            .write_all(
                br#"{
                    "data":{"BINPATCH":{"client":"/data/client.lzma","server":"/data/server.lzma"}},
                    "processors":[{"jar":"example:processor:1.0","classpath":[],"args":["{BINPATCH}"]}],
                    "libraries":[{"name":"example:processor:1.0"}]
                }"#,
            )
            .unwrap();
    archive.start_file("data/client.lzma", options).unwrap();
    archive.write_all(b"client-patch").unwrap();
    archive.start_file("data/server.lzma", options).unwrap();
    archive.write_all(b"server-patch").unwrap();
    let bytes = archive.finish().unwrap().into_inner();

    let parsed =
        parse_installer(bytes, "https://example.com/installer.jar").unwrap();
    assert_eq!(parsed.artifacts.len(), 2);
    assert_eq!(parsed.profile.processors.as_ref().unwrap().len(), 1);
    assert!(parsed.profile.libraries.iter().any(|library| {
        library.name == "example:processor:1.0" && !library.include_in_classpath
    }));
    assert!(parsed.profile.libraries.iter().any(|library| {
        library.name == "example:generated:1.0" && !library.downloadable
    }));
    let data = parsed.profile.data.unwrap();
    assert!(data["BINPATCH"].client.starts_with("[com.axolotl."));
    assert_eq!(
        data[INSTALLER_URL_KEY].client,
        "https://example.com/installer.jar"
    );
}

fn minimal_installer_version(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "inheritsFrom": "1.12.2",
        "releaseTime": "2017-09-18T08:39:46+00:00",
        "time": "2017-09-18T08:39:46+00:00",
        "mainClass": "net.minecraft.launchwrapper.Launch",
        "libraries": [],
        "type": "release"
    })
}

fn installer_fixture(
    version_path: Option<&str>,
    profile: serde_json::Value,
) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default();
    if let Some(version_path) = version_path {
        archive.start_file(version_path, options).unwrap();
        archive
            .write_all(
                serde_json::to_string(&minimal_installer_version(version_path))
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap();
    }
    archive.start_file("install_profile.json", options).unwrap();
    archive
        .write_all(serde_json::to_string(&profile).unwrap().as_bytes())
        .unwrap();
    archive.finish().unwrap().into_inner()
}

#[test]
fn babric_manifest_is_scoped_to_beta_1_7_3_and_uses_babric_profiles() {
    let manifest =
        babric_manifest("b1.7.3", vec!["0.15.6-babric.2".to_string()]);
    assert_eq!(manifest.game_versions[0].id, "b1.7.3");
    let loader = &manifest.game_versions[0].loaders[0];
    assert!(loader.url.contains("meta.babric.dev"));
    assert!(
        loader
            .url
            .contains("loader/b1.7.3/0.15.6-babric.2/profile/json")
    );
    assert_eq!(loader.profile_source, LoaderProfileSource::Babric);
    assert!(loader.fallback_url.is_none());

    let profile: PartialVersionInfo =
        serde_json::from_value(serde_json::json!({
            "id": "babric-loader-0.15.6-babric.2-b1.7.3",
            "inheritsFrom": "b1.7.3",
            "releaseTime": "2026-08-23T00:00:00Z",
            "time": "2026-08-23T00:00:00Z",
            "type": "release",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "libraries": [
                {"name": "babric:intermediary:b1.7.3"},
                {"name": "babric:fabric-loader:0.15.6-babric.2"}
            ],
            "arguments": {"game": ["--demo"]}
        }))
        .unwrap();
    assert_eq!(
        profile.main_class.as_deref(),
        Some("net.fabricmc.loader.impl.launch.knot.KnotClient")
    );
    assert_eq!(profile.inherits_from, "b1.7.3");
    assert!(
        profile
            .libraries
            .iter()
            .any(|library| { library.name == "babric:intermediary:b1.7.3" })
    );
    assert_eq!(
        serde_json::to_value(&profile.arguments).unwrap()["game"],
        serde_json::json!(["--demo"])
    );
}

#[test]
fn babric_uses_the_glass_maven_fallback_when_metadata_is_unavailable() {
    let manifest = babric_manifest(
        BABRIC_GAME_VERSION,
        vec![BABRIC_FALLBACK_LOADER_VERSION.to_string()],
    );
    let loader = &manifest.game_versions[0].loaders[0];

    assert_eq!(manifest.game_versions[0].id, BABRIC_GAME_VERSION);
    assert_eq!(loader.id, BABRIC_FALLBACK_LOADER_VERSION);
    assert!(loader.stable);
    assert!(
        loader
            .url
            .contains("loader/b1.7.3/0.15.6-babric.2/profile/json")
    );

    let unsupported = unsupported_loader_manifest("1.7.10");
    assert_eq!(unsupported.game_versions[0].id, "1.7.10");
    assert!(unsupported.game_versions[0].loaders.is_empty());
}

#[test]
fn babric_profile_libraries_copy_only_babric_lwjgl_from_polyfill() {
    let libraries = babric_lwjgl_libraries(&serde_json::json!({
        "libraries": [
            {"name": "org.lwjgl.lwjgl:lwjgl:2.9.4-babric.1"},
            {"name": "org.lwjgl.lwjgl:lwjgl_util:2.9.4-babric.1"},
            {"name": "org.lwjgl.lwjgl:lwjgl:2.9.0"},
            {"name": "net.java.jinput:jinput:2.0.5"}
        ]
    }))
    .unwrap();
    assert_eq!(libraries.len(), 2);
    assert!(libraries.iter().all(|library| {
        library.name.contains("lwjgl") && library.name.contains("-babric.")
    }));
}

#[test]
fn installer_parser_accepts_mid_legacy_profile_json_path() {
    let bytes = installer_fixture(
        Some("install/1.8.9.json"),
        serde_json::json!({
            "json": "/install/1.8.9.json",
            "libraries": []
        }),
    );

    let parsed = parse_installer(bytes, "https://example.com/forge.jar")
        .expect("mid-legacy installer");
    assert_eq!(parsed.profile.id, "install/1.8.9.json");
}

#[test]
fn installer_parser_accepts_old_inline_version_info() {
    let version_info = minimal_installer_version("1.7.10-Forge");
    let bytes = installer_fixture(
        None,
        serde_json::json!({
            "versionInfo": version_info,
            "libraries": []
        }),
    );

    let parsed = parse_installer(bytes, "https://example.com/forge.jar")
        .expect("old inline installer");
    assert_eq!(parsed.profile.id, "1.7.10-Forge");
}

#[test]
fn installer_parser_accepts_legacy_universal_artifact() {
    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default();
    archive.start_file("install_profile.json", options).unwrap();
    archive
			.write_all(
				serde_json::json!({
					"versionInfo": minimal_installer_version("1.7.10-Forge"),
					"install": {
						"path": "net.minecraftforge:forge:1.7.10-10.13.4.1614-1.7.10",
						"filePath": "/forge-universal.jar"
					}
				})
				.to_string()
				.as_bytes(),
			)
			.unwrap();
    archive.start_file("forge-universal.jar", options).unwrap();
    archive.write_all(b"legacy universal artifact").unwrap();
    let parsed = parse_installer(
        archive.finish().unwrap().into_inner(),
        "https://example.com/forge.jar",
    )
    .unwrap();

    assert!(parsed.profile.libraries.iter().any(|library| {
        library.name == "net.minecraftforge:forge:1.7.10-10.13.4.1614-1.7.10"
            && !library.downloadable
    }));
    assert_eq!(parsed.artifacts.len(), 1);
}

#[test]
fn coordinate_from_embedded_maven_path_preserves_classifier_and_extension() {
    assert_eq!(
        coordinate_from_maven_path(
            "maven/com/example/tool/1.2.3/tool-1.2.3-all.jar"
        ),
        Some("com.example:tool:1.2.3:all@jar".to_string())
    );
    assert_eq!(
        coordinate_from_maven_path(
            "maven/com/example/tool/1.2.3/tool-1.2.3.zip"
        ),
        Some("com.example:tool:1.2.3@zip".to_string())
    );
    assert_eq!(
        coordinate_from_maven_path(
            "maven/com/example/tool/1.2.3/not-an-artifact.jar"
        ),
        None
    );
}

#[test]
fn installer_profile_uses_profile_minecraft_when_version_has_no_inheritance() {
    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default();
    archive.start_file("version.json", options).unwrap();
    archive
			.write_all(
				serde_json::json!({
					"id": "1.12.2-Cleanroom-test",
					"releaseTime": "2026-08-15T20:37:45+00:00",
					"time": "2026-08-15T20:37:45+00:00",
					"mainClass": "top.outlands.foundation.boot.Foundation",
					"javaVersion": {"component": "java-runtime-epsilon", "majorVersion": 25},
					"libraries": [],
					"type": "release"
				})
				.to_string()
				.as_bytes(),
			)
			.unwrap();
    archive.start_file("install_profile.json", options).unwrap();
    archive
        .write_all(br#"{"minecraft":"1.12.2","json":"/version.json"}"#)
        .unwrap();
    let parsed = parse_installer(
        archive.finish().unwrap().into_inner(),
        "https://example.com/cleanroom.jar",
    )
    .unwrap();

    assert_eq!(parsed.profile.inherits_from, "1.12.2");
    assert_eq!(
        parsed
            .profile
            .java_version
            .as_ref()
            .map(|java| java.major_version),
        Some(25)
    );
}

#[test]
fn provider_fixtures_filter_and_parse_new_loaders() {
    assert_eq!(
        optifine_metadata_url(None),
        "https://bmclapi2.bangbang93.com/optifine/versionList"
    );
    assert_eq!(
        optifine_metadata_url(Some("1.20.1")),
        "https://bmclapi2.bangbang93.com/optifine/1.20.1"
    );

    let optifine = optifine_manifest(
        vec![
            OptiFineMetadata {
                mcversion: "1.12.2".to_string(),
                type_: "HD_U".to_string(),
                patch: "G5".to_string(),
            },
            OptiFineMetadata {
                mcversion: "1.20.1".to_string(),
                type_: "HD_U".to_string(),
                patch: "I6_pre1".to_string(),
            },
        ],
        Some("1.12.2"),
    );
    assert_eq!(optifine.game_versions.len(), 1);
    assert_eq!(optifine.game_versions[0].loaders[0].id, "OptiFine_HD_U_G5");

    let unsupported_optifine = optifine_manifest(Vec::new(), Some("26.2"));
    assert_eq!(unsupported_optifine.game_versions.len(), 1);
    assert_eq!(unsupported_optifine.game_versions[0].id, "26.2");
    assert!(unsupported_optifine.game_versions[0].loaders.is_empty());
    assert!(validate_scoped_manifest(unsupported_optifine, "26.2").is_ok());

    let lite_metadata = serde_json::json!({
        "versions": {
            "1.6.4": {
                "artefacts": {
                    "com.mumfrey:liteloader": {
                        "latest": {
                            "tweakClass": "com.mumfrey.liteloader.launch.LiteLoaderTweaker",
                            "libraries": [{"name": "net.minecraft:launchwrapper:1.8"}],
                            "version": "1.6.4_01",
                            "stream": "RELEASE",
                            "timestamp": "1380567800"
                        }
                    }
                }
            },
            "1.12.2": {
                "snapshots": {
                    "com.mumfrey:liteloader": {
                        "latest": {
                            "tweakClass": "com.mumfrey.liteloader.launch.LiteLoaderTweaker",
                            "libraries": [{"name": "net.minecraft:launchwrapper:1.12"}],
                            "version": "1.12.2-SNAPSHOT",
                            "stream": "SNAPSHOT",
                            "timestamp": "1511880271"
                        }
                    }
                }
            }
        }
    });
    let lite = liteloader_manifest(&lite_metadata).unwrap();
    assert_eq!(lite.game_versions.len(), 1);
    assert_eq!(lite.game_versions[0].id, "1.12.2");
    assert_eq!(lite.game_versions[0].loaders[0].id, "1.12.2-SNAPSHOT");
    let profile =
        build_liteloader_profile(&lite_metadata, "1.12.2", "1.12.2-SNAPSHOT")
            .unwrap();
    assert_eq!(
        profile.main_class.as_deref(),
        Some("net.minecraft.launchwrapper.Launch")
    );
    assert!(profile.libraries.iter().any(|library| {
        library.name == "com.mumfrey:liteloader:1.12.2-SNAPSHOT"
    }));
    assert_eq!(
        profile.java_version.as_ref().map(|java| java.major_version),
        Some(8)
    );
    assert_eq!(
        profile
            .java_version
            .as_ref()
            .map(|java| java.component.as_str()),
        Some("jre-legacy")
    );

    let legacy_fabric_profile: PartialVersionInfo =
            serde_json::from_value(serde_json::json!({
                "id": "fabric-loader-0.19.3-1.8.9",
                "inheritsFrom": "1.8.9",
                "releaseTime": "2026-08-19T13:03:54+0000",
                "time": "2026-08-19T13:03:54+0000",
                "type": "release",
                "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
                "libraries": [
                    {"name": "net.legacyfabric:intermediary:1.8.9", "url": "https://maven.legacyfabric.net/"},
                    {"name": "net.fabricmc:fabric-loader:0.19.3", "url": "https://maven.fabricmc.net/"},
                    {"name": "org.lwjgl.lwjgl:lwjgl:2.9.4+legacyfabric.17", "url": "https://maven.legacyfabric.net/"}
                ]
            }))
            .unwrap();
    assert_eq!(
        legacy_fabric_profile.main_class.as_deref(),
        Some("net.fabricmc.loader.impl.launch.knot.KnotClient")
    );
    assert!(legacy_fabric_profile.libraries.iter().any(|library| {
        library.name == "net.legacyfabric:intermediary:1.8.9"
    }));
    assert!(legacy_fabric_profile.arguments.is_none());
    assert!(legacy_fabric_profile.minecraft_arguments.is_none());

    let cleanroom = cleanroom_manifest(vec![GithubRelease {
        tag_name: "0.6.11-alpha".to_string(),
        draft: false,
        prerelease: false,
        assets: vec![GithubReleaseAsset {
            name: "cleanroom-0.6.11-alpha-installer.jar".to_string(),
            browser_download_url: "https://example.com/cleanroom.jar"
                .to_string(),
        }],
    }]);
    assert_eq!(cleanroom.game_versions[0].id, "1.12.2");
    assert_eq!(
        cleanroom.game_versions[0].loaders[0].profile_source,
        LoaderProfileSource::Installer
    );
}

#[test]
fn cleanroom_maven_metadata_builds_installer_manifest() {
    let metadata: MavenMetadata = quick_xml::de::from_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<metadata>
  <groupId>com.cleanroommc</groupId>
  <artifactId>cleanroom</artifactId>
  <versioning>
    <latest>0.6.13-alpha</latest>
    <release>0.6.13-alpha</release>
    <versions>
      <version>0.4.0-alpha</version>
      <version>0.4.0</version>
      <version>0.6.13-alpha</version>
    </versions>
  </versioning>
</metadata>"#,
    )
    .unwrap();

    let manifest = cleanroom_maven_manifest(metadata);
    assert_eq!(manifest.game_versions.len(), 1);
    assert_eq!(manifest.game_versions[0].id, "1.12.2");

    let loaders = &manifest.game_versions[0].loaders;
    // Newest first; the stable 0.4.0 release outranks its alpha twin.
    assert_eq!(
        loaders
            .iter()
            .map(|version| version.id.as_str())
            .collect::<Vec<_>>(),
        ["0.6.13-alpha", "0.4.0", "0.4.0-alpha"]
    );
    assert_eq!(
        loaders[0].url,
        "https://repo.cleanroommc.com/releases/com/cleanroommc/cleanroom/0.6.13-alpha/cleanroom-0.6.13-alpha-installer.jar"
    );
    assert_eq!(loaders[0].profile_source, LoaderProfileSource::Installer);
    assert!(!loaders[0].stable);
    assert!(loaders[1].stable);
    assert!(!loaders[2].stable);
}

#[test]
fn natural_version_order_keeps_latest_first() {
    let mut versions = ["55.1.9", "55.1.13", "55.1.10"];
    versions.sort_by(|left, right| compare_versions(right, left));
    assert_eq!(versions, ["55.1.13", "55.1.10", "55.1.9"]);
}
