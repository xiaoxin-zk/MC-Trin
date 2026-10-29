use super::*;
use crate::launcher::natives::prepare_native_libraries as prepare_test_natives;

async fn client_sha1(bytes: &[u8]) -> String {
    sha1_async(bytes.to_vec().into()).await.expect("hash")
}

#[tokio::test]
async fn an_existing_client_jar_with_the_wrong_contents_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("1.20.1.jar");
    let good = b"a complete client jar".as_slice();
    let truncated = b"a complete client".as_slice();
    tokio::fs::write(&path, truncated).await.unwrap();

    assert_eq!(
        client_jar_is_intact(&path, Some(&client_sha1(good).await))
            .await
            .unwrap(),
        Some(false),
        "a wrong-content cache entry must not be reused"
    );
    assert_eq!(
        client_jar_is_intact(&path, Some(&client_sha1(truncated).await))
            .await
            .unwrap(),
        Some(true),
        "an intact cache entry is reusable"
    );
}

#[tokio::test]
async fn a_client_jar_without_metadata_hash_is_not_strongly_verified() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("1.20.1.jar");
    tokio::fs::write(&path, b"client").await.unwrap();

    assert_eq!(client_jar_is_intact(&path, None).await.unwrap(), None);
    assert_eq!(
        client_jar_is_intact(&path, Some("")).await.unwrap(),
        None,
        "an empty hash must not be reported as a verified file"
    );
}

#[tokio::test]
async fn a_corrupt_client_jar_is_moved_aside_instead_of_deleted() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("1.20.1.jar");
    tokio::fs::write(&path, b"broken").await.unwrap();

    let quarantine = move_corrupt_client(&path).await.expect("quarantine file");

    assert!(!path.exists(), "the broken file leaves its original path");
    assert_eq!(tokio::fs::read(&quarantine).await.unwrap(), b"broken");
    assert!(
        quarantine.starts_with(directory.path().join(".corrupt")),
        "the file stays beside its version directory"
    );
}

fn urls(values: &[&str]) -> Option<Vec<String>> {
    Some(values.iter().map(|value| (*value).to_string()).collect())
}

fn version_info_with_java(
    java: Option<d::minecraft::JavaVersion>,
) -> GameVersionInfo {
    let mut info: GameVersionInfo = serde_json::from_value(serde_json::json!({
            "assetIndex": {"id": "1.12", "sha1": "", "size": 0, "totalSize": 0, "url": ""},
            "assets": "1.12",
            "downloads": {},
            "id": "1.12.2",
            "libraries": [],
            "mainClass": "net.minecraft.client.main.Main",
            "minimumLauncherVersion": 21,
            "releaseTime": "2026-01-01T00:00:00+00:00",
            "time": "2026-01-01T00:00:00+00:00",
            "type": "release"
        }))
        .unwrap();
    info.java_version = java;
    info
}

#[test]
fn cleanroom_without_java_version_is_raised_to_java_21() {
    let mut info = version_info_with_java(None);
    assert!(normalize_version_info(
        ModLoader::Cleanroom,
        "1.12.2",
        &mut info,
        "network",
    ));
    let java = info.java_version.expect("java version pinned");
    assert_eq!(java.major_version, 21);
    assert_eq!(java.component, "java-runtime-delta");
}

#[tokio::test]
async fn cached_loader_timestamps_are_corrected_online_and_offline() {
    let temp = tempfile::tempdir().unwrap();
    let directories = crate::state::DirectoryInfo {
        settings_dir: temp.path().join("settings"),
        config_dir: temp.path().join("config"),
        app_identifier: "test".to_string(),
    };
    std::fs::create_dir_all(directories.instances_dir()).unwrap();
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    let state = crate::state::test_state(directories, pool).await.unwrap();
    let version: GameVersion = serde_json::from_value(serde_json::json!({
        "id": "1.12.2", "type": "release", "url": "invalid://no-network",
        "releaseTime": "2017-09-18T08:39:46Z", "time": "2021-12-15T15:04:05Z",
        "sha1": "", "complianceLevel": 0
    }))
    .unwrap();
    let loader: LoaderVersion = serde_json::from_value(serde_json::json!({
        "id": "test-loader", "stable": true, "url": "invalid://no-network"
    }))
    .unwrap();
    let path = state
        .directories
        .version_dir("1.12.2-test-loader")
        .join("1.12.2-test-loader.json");
    for offline in [false, true] {
        let mut info = version_info_with_java(None);
        info.id = "1.12.2-test-loader".to_string();
        write_version_info(&path, serde_json::to_vec(&info).unwrap())
            .await
            .unwrap();
        if offline {
            io::write(derived_version_cache_marker_path(&path), "3")
                .await
                .unwrap();
        } else {
            write_derived_version_cache_marker(&path).await.unwrap();
        }
        let mut loaded = if offline {
            load_local_version_info(
                &state,
                &version,
                ModLoader::Forge,
                Some(&loader),
            )
            .await
        } else {
            download_version_info(
                &state,
                &version,
                ModLoader::Forge,
                Some(&loader),
                None,
                None,
                None,
            )
            .await
        }
        .unwrap();
        assert_eq!(loaded.release_time, version.release_time);
        assert_eq!(loaded.time, version.time);
        let saved: GameVersionInfo =
            serde_json::from_slice(&io::read(&path).await.unwrap()).unwrap();
        assert_eq!(saved.release_time, version.release_time);
        assert_eq!(saved.time, version.time);
        assert!(!normalize_version_timestamps(&version, &mut loaded));
        assert_eq!(derived_version_cache_is_current(&path).await, !offline);
    }
}

#[tokio::test]
async fn installed_forge_profile_is_validated_without_loader_catalogs() {
    let temp = tempfile::tempdir().unwrap();
    let directories = crate::state::DirectoryInfo {
        settings_dir: temp.path().join("settings"),
        config_dir: temp.path().join("config"),
        app_identifier: "test".into(),
    };
    std::fs::create_dir_all(directories.instances_dir()).unwrap();
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    let state = crate::state::test_state(directories, pool).await.unwrap();
    let version: GameVersion = serde_json::from_value(serde_json::json!({
        "id":"1.20.1", "type":"release", "url":"invalid://no-network",
        "releaseTime":"2023-06-12T13:25:51Z", "time":"2023-06-12T13:25:51Z",
        "sha1":"", "complianceLevel":1
    }))
    .unwrap();
    let loader: LoaderVersion = serde_json::from_value(serde_json::json!({
        "id":"47.4.28", "stable":false, "url":""
    }))
    .unwrap();
    let path = state
        .directories
        .version_dir("1.20.1-47.4.28")
        .join("1.20.1-47.4.28.json");
    let mut info = version_info_with_java(None);
    info.id = "1.20.1-47.4.28".into();
    info.main_class = "cpw.mods.bootstraplauncher.BootstrapLauncher".into();
    info.libraries = vec![
        serde_json::from_value(serde_json::json!({
            "name":"net.minecraftforge:forge:1.20.1-47.4.28:universal"
        }))
        .unwrap(),
    ];
    assert!(
        try_load_launch_version_info(
            &state,
            &version,
            ModLoader::Forge,
            Some(&loader)
        )
        .await
        .is_none()
    );
    write_version_info(&path, serde_json::to_vec(&info).unwrap())
        .await
        .unwrap();
    assert!(
        try_load_launch_version_info(
            &state,
            &version,
            ModLoader::Forge,
            Some(&loader)
        )
        .await
        .is_none()
    );
    write_derived_version_cache_marker(&path).await.unwrap();
    let loaded = try_load_launch_version_info(
        &state,
        &version,
        ModLoader::Forge,
        Some(&loader),
    )
    .await
    .unwrap();
    assert_eq!(loaded.id, "1.20.1-47.4.28");
    assert_eq!(loaded.main_class, info.main_class);
    assert_eq!(loaded.release_time, version.release_time);
    let (selected, launch_info) = crate::launcher::load_managed_launch_profile(
        &state,
        &version,
        ModLoader::Forge,
        Some("47.4.28"),
        false,
    )
    .await
    .unwrap();
    assert_eq!(selected.unwrap().id, "47.4.28");
    assert_eq!(launch_info.id, "1.20.1-47.4.28");
    for bad in ["{", "{}"] {
        write_version_info(&path, bad.as_bytes().to_vec())
            .await
            .unwrap();
        assert!(
            try_load_launch_version_info(
                &state,
                &version,
                ModLoader::Forge,
                Some(&loader)
            )
            .await
            .is_none()
        );
    }
    for variant in 0..4 {
        let mut invalid: GameVersionInfo =
            serde_json::from_value(serde_json::to_value(&info).unwrap())
                .unwrap();
        match variant {
            0 => invalid.id = "1.20.1-47.4.22".into(),
            1 => invalid.main_class.clear(),
            2 => invalid.libraries.clear(),
            _ => {
                invalid.libraries[0].name =
                    "net.minecraftforge:forge:1.20.1-47.4.22:universal".into()
            }
        }
        write_version_info(&path, serde_json::to_vec(&invalid).unwrap())
            .await
            .unwrap();
        assert!(
            try_load_launch_version_info(
                &state,
                &version,
                ModLoader::Forge,
                Some(&loader)
            )
            .await
            .is_none(),
            "variant {variant}"
        );
    }
}

#[test]
fn cleanroom_installer_declared_java_is_preserved() {
    let mut info = version_info_with_java(Some(d::minecraft::JavaVersion {
        component: "java-runtime-epsilon".to_string(),
        major_version: 25,
    }));
    assert!(!normalize_version_info(
        ModLoader::Cleanroom,
        "1.12.2",
        &mut info,
        "network",
    ));
    assert_eq!(
        info.java_version.as_ref().map(|java| java.major_version),
        Some(25)
    );
}

#[test]
fn vanilla_stays_on_inherited_java_runtime() {
    let mut info = version_info_with_java(None);
    assert!(!normalize_version_info(
        ModLoader::Vanilla,
        "1.12.2",
        &mut info,
        "network",
    ));
    assert!(info.java_version.is_none());
}

#[test]
fn batch_assets_coalesce_duplicate_objects_and_keep_legacy_targets() {
    let destination = PathBuf::from("assets/objects/ab/abcdef");
    let first_legacy = PathBuf::from("resources/first");
    let second_legacy = PathBuf::from("resources/second");
    let make_item = |legacy_destinations| H2BatchAsset {
        url: "https://resources.download.minecraft.net/ab/abcdef".into(),
        destination: destination.clone(),
        legacy_destinations,
        sha1: "abcdef".into(),
        size: 42,
        logical_items: 1,
    };

    let items = coalesce_batch_assets(vec![
        make_item(vec![first_legacy.clone()]),
        make_item(vec![second_legacy.clone(), first_legacy.clone()]),
    ]);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].logical_items, 2);
    assert_eq!(
        items[0].legacy_destinations,
        vec![first_legacy, second_legacy]
    );
}

#[test]
fn batch_assets_do_not_coalesce_different_integrity_contracts() {
    let destination = PathBuf::from("assets/objects/ab/object");
    let make_item = |sha1: &str, size| H2BatchAsset {
        url: format!("https://resources.download.minecraft.net/ab/{sha1}"),
        destination: destination.clone(),
        legacy_destinations: Vec::new(),
        sha1: sha1.into(),
        size,
        logical_items: 1,
    };

    assert_eq!(
        coalesce_batch_assets(vec![
            make_item("first", 42),
            make_item("second", 42),
            make_item("first", 43),
        ])
        .len(),
        3
    );
}

#[test]
fn fallback_assets_coalesce_aliases_and_keep_legacy_targets() {
    let destination = PathBuf::from("assets/objects/85/hash");
    let first_legacy = PathBuf::from("resources/sound/liquid/splash2.ogg");
    let second_legacy = PathBuf::from("resources/sounds/liquid/splash2.ogg");
    let make_item = |name: &str, legacy_resource_paths: Vec<PathBuf>| {
        FallbackAsset {
                name: name.into(),
                hash: "857abbbfb58186c2f1b5510a4072630950e518f6".into(),
                size: 36_747,
            url: "https://resources.download.minecraft.net/85/857abbbfb58186c2f1b5510a4072630950e518f6".into(),
            resource_path: destination.clone(),
                legacy_resource_paths,
                logical_items: 1,
            }
    };

    let items = coalesce_fallback_assets(vec![
        make_item("sound/liquid/splash2.ogg", vec![first_legacy.clone()]),
        make_item(
            "sounds/liquid/splash2.ogg",
            vec![second_legacy.clone(), first_legacy.clone()],
        ),
    ]);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].logical_items, 2);
    assert_eq!(
        items[0].legacy_resource_paths,
        vec![first_legacy, second_legacy]
    );
}

#[test]
fn fallback_assets_do_not_coalesce_different_integrity_contracts() {
    let destination = PathBuf::from("assets/objects/85/object");
    let make_item = |hash: &str, size| FallbackAsset {
        name: hash.into(),
        hash: hash.into(),
        size,
        url: format!("https://resources.download.minecraft.net/85/{hash}"),
        resource_path: destination.clone(),
        legacy_resource_paths: Vec::new(),
        logical_items: 1,
    };

    assert_eq!(
        coalesce_fallback_assets(vec![
            make_item("first", 42),
            make_item("second", 42),
            make_item("first", 43),
        ])
        .len(),
        3
    );
}

#[test]
fn asset_fast_path_requires_a_regular_file_with_expected_size() {
    let directory = tempfile::tempdir().unwrap();
    let valid = directory.path().join("valid");
    let truncated = directory.path().join("truncated");
    let folder = directory.path().join("folder");
    std::fs::write(&valid, b"asset").unwrap();
    std::fs::write(&truncated, b"as").unwrap();
    std::fs::create_dir(&folder).unwrap();

    assert!(asset_file_is_usable(&valid, 5));
    assert!(!asset_file_is_usable(&valid, 4));
    assert!(!asset_file_is_usable(&truncated, 5));
    assert!(!asset_file_is_usable(&folder, 0));
    assert!(!asset_file_is_usable(&directory.path().join("missing"), 5));
}

#[tokio::test]
async fn cached_artifact_with_wrong_size_is_not_reused() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("asset");
    std::fs::write(&destination, b"bad").unwrap();

    assert_eq!(
        classify_local_artifact(
            None,
            &destination,
            Path::new("assets/objects/00/hash"),
            Some("unused"),
            Some(5),
        )
        .await
        .unwrap(),
        ArtifactAvailability::NetworkRequired,
    );
}

#[tokio::test]
async fn local_asset_availability_distinguishes_reusable_and_missing() {
    let directory = tempfile::tempdir().unwrap();
    let local_root = directory.path().join(".minecraft");
    let relative = Path::new("assets/objects/ab/hash");
    let candidate = local_root.join(relative);
    io::create_dir_all(candidate.parent().unwrap())
        .await
        .unwrap();
    io::write(&candidate, "asset").await.unwrap();
    let source = LocalRuntimeSource { root: local_root };
    let destination = directory.path().join("destination");

    assert_eq!(
        classify_local_artifact(
            Some(&source),
            &destination,
            relative,
            Some("0000000000000000000000000000000000000000"),
            Some(5),
        )
        .await
        .unwrap(),
        ArtifactAvailability::NetworkRequired
    );
    assert_eq!(
        classify_local_artifact(
            Some(&source),
            &destination,
            relative,
            Some("05fac94380a70241f23780e7aef62b190894238f"),
            Some(5),
        )
        .await
        .unwrap(),
        ArtifactAvailability::LocalReusable
    );
}

#[test]
fn liteloader_library_detection_is_coordinate_specific() {
    assert!(is_liteloader_library(
        "com.mumfrey:liteloader:1.12.2-SNAPSHOT"
    ));
    assert!(!is_liteloader_library("example:liteloader:1.12.2-SNAPSHOT"));
    assert!(!is_liteloader_library("com.mumfrey:other:1.0"));
    assert!(!is_liteloader_library("com.mumfrey:liteloader"));
}

#[test]
fn legacy_native_library_uses_platform_classifier_without_downloads_block() {
    let library: Library = serde_json::from_value(serde_json::json!({
        "name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4+legacyfabric.17",
        "url": "https://maven.legacyfabric.net/",
        "natives": {
            "linux": "natives-linux",
            "osx": "natives-osx",
            "windows": "natives-windows"
        },
        "checksums": ["05fac94380a70241f23780e7aef62b190894238f"]
    }))
    .unwrap();

    assert!(library.natives_os_key_and_classifiers("x86_64").is_none());
    let classifier = library_native_classifier(&library, "x86_64").unwrap();
    assert_eq!(
        legacy_library_sha1(&library),
        Some("05fac94380a70241f23780e7aef62b190894238f")
    );
    assert_eq!(
        legacy_library_content_validation("lwjgl-platform.jar"),
        ContentValidation::Jar
    );
    let artifact_path =
        classified_library_artifact_path(&library.name, &classifier).unwrap();
    assert_eq!(
        artifact_path,
        format!(
            "org/lwjgl/lwjgl/lwjgl-platform/2.9.4+legacyfabric.17/lwjgl-platform-2.9.4+legacyfabric.17-{classifier}.jar"
        )
    );
    assert_eq!(
        legacy_library_download_urls(library.url.as_deref(), &artifact_path,)
            .unwrap()[0],
        format!("https://maven.legacyfabric.net/{artifact_path}")
    );
}

#[test]
fn native_library_with_legacy_repository_keeps_java_artifact_task() {
    let library: Library = serde_json::from_value(serde_json::json!({
        "name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4",
        "url": "https://libraries.minecraft.net/",
        "natives": {
            "windows": "natives-windows"
        }
    }))
    .unwrap();

    assert!(needs_java_artifact(&library));
}

#[test]
fn processor_dependency_without_classpath_flag_keeps_java_artifact_task() {
    let library: Library = serde_json::from_value(serde_json::json!({
            "name": "net.neoforged.installertools:installertools:2.1.2",
            "downloads": {"artifact": {
                "url": "https://maven.neoforged.net/releases/net/neoforged/installertools/installertools/2.1.2/installertools-2.1.2.jar",
                "sha1": "", "size": 1
            }},
            "include_in_classpath": false,
            "downloadable": true
        }))
        .unwrap();

    assert!(needs_java_artifact(&library));
    assert!(java_artifact_applies(&library, "x86_64", true));
}

#[test]
fn legacy_launchwrapper_without_download_metadata_is_downloaded() {
    let library: Library = serde_json::from_value(serde_json::json!({
        "name": "net.minecraft:launchwrapper:1.12"
    }))
    .unwrap();

    let artifact_path = d::get_path_from_artifact(&library.name).unwrap();
    assert!(needs_java_artifact(&library));
    assert_eq!(
        legacy_library_download_urls(library.url.as_deref(), &artifact_path)
            .unwrap(),
        vec![format!("{LIBRARIES_MAVEN}/{artifact_path}")]
    );
}

#[test]
fn java_artifact_rules_are_applied_before_path_deduplication() {
    let blocked: Library = serde_json::from_value(serde_json::json!({
        "name": "example:library:1.0",
        "rules": [{"action": "allow", "features": {"is_demo_user": true}}],
        "downloads": {"artifact": {
            "url": "https://example.invalid/library.jar",
            "sha1": "", "size": 1
        }}
    }))
    .unwrap();
    let allowed: Library = serde_json::from_value(serde_json::json!({
        "name": "example:library:1.0",
        "downloads": {"artifact": {
            "url": "https://example.invalid/library.jar",
            "sha1": "", "size": 1
        }}
    }))
    .unwrap();

    assert!(!java_artifact_applies(&blocked, "x86_64", true));
    assert!(java_artifact_applies(&allowed, "x86_64", true));
}

#[test]
fn empty_legacy_repository_is_not_a_java_artifact_source() {
    let library: Library = serde_json::from_value(serde_json::json!({
        "name": "example:native:1.0",
        "url": "",
        "natives": {"windows": "natives-windows"}
    }))
    .unwrap();

    assert!(!needs_java_artifact(&library));
}

#[test]
fn forge_classifier_native_library_uses_its_declared_artifact_path() {
    let library: Library = serde_json::from_value(serde_json::json!({
        "name": "org.lwjgl:lwjgl:3.2.2:natives-windows"
    }))
    .unwrap();

    assert!(is_native_library(&library));
    assert_eq!(
        library_native_classifier(&library, "x86_64"),
        Some("natives-windows".to_string())
    );
    assert_eq!(
        native_library_artifact_path(&library, "natives-windows").unwrap(),
        "org/lwjgl/lwjgl/3.2.2/lwjgl-3.2.2-natives-windows.jar"
    );
}

#[tokio::test]
async fn writing_version_info_creates_missing_parent_directory() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join("meta/versions/1.21.11-21.11.44/1.21.11-21.11.44.json");

    assert!(!path.parent().unwrap().exists());
    write_version_info(&path, br#"{"id":"1.21.11-21.11.44"}"#.to_vec())
        .await
        .unwrap();

    assert_eq!(
        io::read(&path).await.unwrap(),
        br#"{"id":"1.21.11-21.11.44"}"#
    );
}

#[tokio::test]
async fn derived_version_cache_requires_current_format_marker() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("1.20.1-47.2.20.json");
    write_version_info(&path, b"{}".to_vec()).await.unwrap();

    assert!(!derived_version_cache_is_current(&path).await);
    io::write(derived_version_cache_marker_path(&path), "3")
        .await
        .unwrap();
    assert!(!derived_version_cache_is_current(&path).await);

    write_derived_version_cache_marker(&path).await.unwrap();
    assert!(derived_version_cache_is_current(&path).await);
}

#[test]
fn legacy_launcher_meta_maven_uses_canonical_repositories() {
    assert_eq!(
        legacy_library_download_urls(
            Some("https://launcher-meta.modrinth.com/maven/"),
            "net/fabricmc/intermediary/1.21.1/intermediary-1.21.1.jar",
        ),
        urls(&[
            "https://maven.fabricmc.net/net/fabricmc/intermediary/1.21.1/intermediary-1.21.1.jar",
            "https://launcher-meta.modrinth.com/maven/net/fabricmc/intermediary/1.21.1/intermediary-1.21.1.jar",
            "https://libraries.minecraft.net/net/fabricmc/intermediary/1.21.1/intermediary-1.21.1.jar",
        ]),
    );
    assert_eq!(
        legacy_library_download_urls(
            Some("https://launcher-meta.modrinth.com/maven/"),
            "org/ow2/asm/asm/9.10.1/asm-9.10.1.jar",
        ),
        urls(&[
            "https://repo.maven.apache.org/maven2/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar",
            "https://launcher-meta.modrinth.com/maven/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar",
            "https://libraries.minecraft.net/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar",
        ]),
    );
    assert_eq!(
        legacy_library_download_urls(
            Some("https://launcher-meta.modrinth.com/maven/"),
            "com/modrinth/daedalus/forge-installer-extracts/1.20.1-47.4.20/forge-installer-extracts-1.20.1-47.4.20-client.lzma",
        ),
        urls(&[
            "https://launcher-meta.modrinth.com/maven/com/modrinth/daedalus/forge-installer-extracts/1.20.1-47.4.20/forge-installer-extracts-1.20.1-47.4.20-client.lzma",
            "https://libraries.minecraft.net/com/modrinth/daedalus/forge-installer-extracts/1.20.1-47.4.20/forge-installer-extracts-1.20.1-47.4.20-client.lzma",
        ]),
    );
    assert_eq!(
        legacy_library_content_validation("library.jar"),
        ContentValidation::Jar,
    );
    assert_eq!(
        legacy_library_content_validation("library.lzma"),
        ContentValidation::None,
    );
}

#[test]
fn legacy_scala_libraries_use_maven_central() {
    let artifact_path = "org/scala-lang/plugins/scala-continuations-library_2.11/1.0.2/scala-continuations-library_2.11-1.0.2.jar";
    let expected = urls(&[
        "https://repo.maven.apache.org/maven2/org/scala-lang/plugins/scala-continuations-library_2.11/1.0.2/scala-continuations-library_2.11-1.0.2.jar",
        "https://launcher-meta.modrinth.com/maven/org/scala-lang/plugins/scala-continuations-library_2.11/1.0.2/scala-continuations-library_2.11-1.0.2.jar",
        "https://libraries.minecraft.net/org/scala-lang/plugins/scala-continuations-library_2.11/1.0.2/scala-continuations-library_2.11-1.0.2.jar",
    ]);

    assert_eq!(
        legacy_library_download_urls(
            Some("https://launcher-meta.modrinth.com/maven"),
            artifact_path,
        ),
        expected,
    );
    assert_eq!(
        legacy_library_download_urls(
            Some("https://launcher-meta.modrinth.com/maven/"),
            artifact_path,
        ),
        expected,
    );
}

#[test]
fn historical_forge_scala_modules_keep_archived_coordinates() {
    let library: Library = serde_json::from_value(serde_json::json!({
        "name": "org.scala-lang:scala-xml_2.11:1.0.2",
        "url": "https://launcher-meta.modrinth.com/maven/",
        "checksums": [
            "7a80ec00aec122fba7cd4e0d4cdd87ff7e4cb6d0",
            "62736b01689d56b6d09a0164b7ef9da2b0b9633d"
        ]
    }))
    .unwrap();
    let artifact_path = d::get_path_from_artifact(&library.name).unwrap();

    assert_eq!(
        artifact_path,
        "org/scala-lang/scala-xml_2.11/1.0.2/scala-xml_2.11-1.0.2.jar",
    );
    assert_eq!(
        legacy_library_sha1(&library),
        Some("7a80ec00aec122fba7cd4e0d4cdd87ff7e4cb6d0"),
    );

    for artifact_path in [
        "org/scala-lang/scala-parser-combinators_2.11/1.0.1/scala-parser-combinators_2.11-1.0.1.jar",
        "org/scala-lang/scala-swing_2.11/1.0.1/scala-swing_2.11-1.0.1.jar",
        "org/scala-lang/scala-xml_2.11/1.0.2/scala-xml_2.11-1.0.2.jar",
    ] {
        assert_eq!(
            legacy_library_download_urls(
                Some("https://launcher-meta.modrinth.com/maven/"),
                artifact_path,
            ),
            Some(vec![
                format!("{FORGE_MAVEN}/{artifact_path}"),
                format!("{LAUNCHER_META_MAVEN}/{artifact_path}"),
                format!("{LIBRARIES_MAVEN}/{artifact_path}"),
            ]),
        );
    }
}

#[test]
fn legacy_forge_maven_central_groups_use_maven_central() {
    for artifact_path in [
        "org/jline/jline/3.5.1/jline-3.5.1.jar",
        "jline/jline/2.13/jline-2.13.jar",
        "net/java/dev/jna/jna/4.4.0/jna-4.4.0.jar",
        "com/typesafe/akka/akka-actor_2.11/2.3.3/akka-actor_2.11-2.3.3.jar",
        "com/typesafe/config/1.2.1/config-1.2.1.jar",
    ] {
        assert_eq!(
            legacy_library_download_urls(
                Some("https://launcher-meta.modrinth.com/maven"),
                artifact_path,
            ),
            Some(vec![
                format!("{MAVEN_CENTRAL}/{artifact_path}"),
                format!("{LAUNCHER_META_MAVEN}/{artifact_path}"),
                format!("{LIBRARIES_MAVEN}/{artifact_path}"),
            ]),
        );
    }
}

#[test]
fn legacy_maven_download_uses_trusted_declared_paths() {
    assert_eq!(
        legacy_library_download_urls(
            Some("https://launcher-meta.modrinth.com/maven/"),
            "example/unknown/1/unknown-1.jar",
        ),
        urls(&[
            "https://launcher-meta.modrinth.com/maven/example/unknown/1/unknown-1.jar",
            "https://libraries.minecraft.net/example/unknown/1/unknown-1.jar",
        ]),
    );
    assert_eq!(
        legacy_library_download_urls(
            Some("https://maven.minecraftforge.net"),
            "example/unknown/1/unknown-1.jar",
        ),
        urls(&[
            "https://maven.minecraftforge.net/example/unknown/1/unknown-1.jar",
            "https://libraries.minecraft.net/example/unknown/1/unknown-1.jar",
        ]),
    );
    assert_eq!(
        legacy_library_download_urls(
            Some("https://example.invalid/maven"),
            "net/fabricmc/intermediary/1.21.1/intermediary-1.21.1.jar",
        ),
        urls(&[
            "https://example.invalid/maven/net/fabricmc/intermediary/1.21.1/intermediary-1.21.1.jar",
            "https://maven.fabricmc.net/net/fabricmc/intermediary/1.21.1/intermediary-1.21.1.jar",
            "https://libraries.minecraft.net/net/fabricmc/intermediary/1.21.1/intermediary-1.21.1.jar",
        ]),
    );
    for repository in [
        "http://example.invalid/maven",
        "https://user@example.invalid/maven",
        "https://example.invalid/maven?token=secret",
    ] {
        assert_eq!(
            legacy_library_download_urls(
                Some(repository),
                "example/unknown/1/unknown-1.jar",
            ),
            None,
        );
    }
    for artifact_path in [
        "/example/unknown.jar",
        "../example/unknown.jar",
        "example\\unknown.jar",
    ] {
        assert_eq!(
            legacy_library_download_urls(
                Some("https://example.invalid/maven"),
                artifact_path,
            ),
            None,
        );
    }
}

fn write_native_archive(path: &Path, entries: &[(&str, &[u8])]) {
    use std::io::Write;

    let file = std::fs::File::create(path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    for (name, contents) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(contents).unwrap();
    }
    writer.finish().unwrap();
}

/// Platform key and native classifier matching the test binary's target.
/// Native resolution derives the platform from the build target, so the
/// fixtures must agree with it instead of assuming Windows.
fn native_platform() -> (&'static str, &'static str) {
    if cfg!(target_os = "windows") {
        ("windows", "natives-windows")
    } else if cfg!(target_os = "macos") {
        ("osx", "natives-macos")
    } else {
        ("linux", "natives-linux")
    }
}

fn modern_native_library(sha1: &str) -> Library {
    let (os_key, classifier) = native_platform();
    let mut natives = serde_json::Map::new();
    natives.insert(
        os_key.to_string(),
        serde_json::Value::String(classifier.to_string()),
    );
    let mut classifiers = serde_json::Map::new();
    classifiers.insert(
        classifier.to_string(),
        serde_json::json!({
            "sha1": sha1,
            "size": 64,
            "url": "https://example.com/natives.jar"
        }),
    );
    serde_json::from_value(serde_json::json!({
        "name": "org.lwjgl:lwjgl-platform:3.2.1",
        "natives": natives,
        "downloads": { "classifiers": classifiers }
    }))
    .unwrap()
}

#[tokio::test]
async fn missing_natives_are_restored_from_cached_archives() {
    let directory = tempfile::tempdir().unwrap();
    let natives_root = directory.path().join("natives");
    let libraries_dir = directory.path().join("libraries");
    let caches_dir = directory.path().join("caches");
    let cache = caches_dir.join("minecraft-natives").join("deadbeef.jar");
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    write_native_archive(
        &cache,
        &[("lwjgl.dll", b"native-binary"), ("sub/inner.dll", b"inner")],
    );

    let libraries = [modern_native_library("deadbeef")];
    prepare_test_natives(
        &natives_root,
        &libraries_dir,
        &caches_dir,
        &libraries,
        "1.16.5",
        "x86_64",
        true,
    )
    .await
    .unwrap();

    let natives_dir = natives_root.join("1.16.5");
    assert_eq!(
        std::fs::read(natives_dir.join("lwjgl.dll")).unwrap(),
        b"native-binary"
    );
    assert_eq!(
        std::fs::read(natives_dir.join("sub/inner.dll")).unwrap(),
        b"inner"
    );

    // Same-length corruption must be repaired on later launches.
    std::fs::write(natives_dir.join("lwjgl.dll"), b"tampered").unwrap();
    prepare_test_natives(
        &natives_root,
        &libraries_dir,
        &caches_dir,
        &libraries,
        "1.16.5",
        "x86_64",
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(natives_dir.join("lwjgl.dll")).unwrap(),
        b"native-binary"
    );
}

#[tokio::test]
async fn only_missing_or_zero_byte_native_entries_are_restored() {
    let directory = tempfile::tempdir().unwrap();
    let natives_root = directory.path().join("natives");
    let libraries_dir = directory.path().join("libraries");
    let caches_dir = directory.path().join("caches");
    let cache = caches_dir.join("minecraft-natives").join("deadbeef.jar");
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    write_native_archive(
        &cache,
        &[
            ("a.dll", b"from-archive"),
            ("b.dll", b"repaired"),
            ("c.txt", b"added"),
        ],
    );

    let natives_dir = natives_root.join("1.16.5");
    std::fs::create_dir_all(&natives_dir).unwrap();
    std::fs::write(natives_dir.join("a.dll"), b"custom").unwrap();
    std::fs::write(natives_dir.join("b.dll"), b"").unwrap();

    let libraries = [modern_native_library("deadbeef")];
    prepare_test_natives(
        &natives_root,
        &libraries_dir,
        &caches_dir,
        &libraries,
        "1.16.5",
        "x86_64",
        true,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read(natives_dir.join("a.dll")).unwrap(),
        b"from-archive"
    );
    assert_eq!(
        std::fs::read(natives_dir.join("b.dll")).unwrap(),
        b"repaired"
    );
    assert_eq!(std::fs::read(natives_dir.join("c.txt")).unwrap(), b"added");
}

#[tokio::test]
async fn native_entries_escaping_natives_directory_are_ignored() {
    let directory = tempfile::tempdir().unwrap();
    let natives_root = directory.path().join("natives");
    let libraries_dir = directory.path().join("libraries");
    let caches_dir = directory.path().join("caches");
    let cache = caches_dir.join("minecraft-natives").join("deadbeef.jar");
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    write_native_archive(
        &cache,
        &[("../evil.dll", b"boom"), ("ok.dll", b"fine")],
    );

    let libraries = [modern_native_library("deadbeef")];
    prepare_test_natives(
        &natives_root,
        &libraries_dir,
        &caches_dir,
        &libraries,
        "1.16.5",
        "x86_64",
        true,
    )
    .await
    .unwrap();

    let natives_dir = natives_root.join("1.16.5");
    assert_eq!(std::fs::read(natives_dir.join("ok.dll")).unwrap(), b"fine");
    assert!(!natives_root.join("evil.dll").exists());
    assert!(!directory.path().join("evil.dll").exists());
}

#[tokio::test]
async fn missing_native_archive_reports_actionable_error() {
    let directory = tempfile::tempdir().unwrap();
    let natives_root = directory.path().join("natives");
    let libraries_dir = directory.path().join("libraries");
    let caches_dir = directory.path().join("caches");

    let libraries = [modern_native_library("nonexistent")];
    let error = prepare_test_natives(
        &natives_root,
        &libraries_dir,
        &caches_dir,
        &libraries,
        "1.16.5",
        "x86_64",
        true,
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Repair the instance while online")
    );
}

#[tokio::test]
async fn missing_native_archive_with_extracted_natives_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let natives_root = directory.path().join("natives");
    let libraries_dir = directory.path().join("libraries");
    let caches_dir = directory.path().join("caches");

    let natives_dir = natives_root.join("1.16.5");
    std::fs::create_dir_all(&natives_dir).unwrap();
    std::fs::write(natives_dir.join("lwjgl.dll"), b"native-binary").unwrap();

    let libraries = [modern_native_library("nonexistent")];
    let error = prepare_test_natives(
        &natives_root,
        &libraries_dir,
        &caches_dir,
        &libraries,
        "1.16.5",
        "x86_64",
        true,
    )
    .await
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("Repair the instance while online")
    );
}

#[tokio::test]
async fn missing_cached_archive_falls_back_to_classified_library_archive() {
    let directory = tempfile::tempdir().unwrap();
    let natives_root = directory.path().join("natives");
    let libraries_dir = directory.path().join("libraries");
    let caches_dir = directory.path().join("caches");
    let (_, classifier) = native_platform();
    let archive = libraries_dir.join(format!(
        "org/lwjgl/lwjgl-platform/3.2.1/lwjgl-platform-3.2.1-{classifier}.jar"
    ));
    std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
    write_native_archive(&archive, &[("lwjgl.dll", b"from-libraries")]);

    let libraries = [modern_native_library("deadbeef")];
    prepare_test_natives(
        &natives_root,
        &libraries_dir,
        &caches_dir,
        &libraries,
        "1.16.5",
        "x86_64",
        true,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read(natives_root.join("1.16.5").join("lwjgl.dll")).unwrap(),
        b"from-libraries"
    );
}

#[tokio::test]
async fn legacy_native_libraries_restore_from_libraries_directory() {
    let directory = tempfile::tempdir().unwrap();
    let natives_root = directory.path().join("natives");
    let libraries_dir = directory.path().join("libraries");
    let caches_dir = directory.path().join("caches");
    let (_, classifier) = native_platform();
    let archive = libraries_dir.join(format!(
            "org/lwjgl/lwjgl/lwjgl-platform/2.9.0/lwjgl-platform-2.9.0-{classifier}.jar"
        ));
    std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
    write_native_archive(&archive, &[("lwjgl.dll", b"legacy-binary")]);

    let library: Library = serde_json::from_value(serde_json::json!({
        "name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.0",
        "natives": {
            "linux": "natives-linux",
            "osx": "natives-osx",
            "windows": "natives-windows"
        }
    }))
    .unwrap();
    prepare_test_natives(
        &natives_root,
        &libraries_dir,
        &caches_dir,
        &[library],
        "1.6.4-9.11.1.1345",
        "x86_64",
        true,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read(natives_root.join("1.6.4-9.11.1.1345").join("lwjgl.dll"))
            .unwrap(),
        b"legacy-binary"
    );
}

#[tokio::test]
async fn non_native_libraries_do_not_trigger_repairs() {
    let directory = tempfile::tempdir().unwrap();
    let natives_root = directory.path().join("natives");
    let libraries_dir = directory.path().join("libraries");
    let caches_dir = directory.path().join("caches");

    let library: Library = serde_json::from_value(serde_json::json!({
        "name": "net.sf.jopt-simple:jopt-simple:4.5"
    }))
    .unwrap();
    prepare_test_natives(
        &natives_root,
        &libraries_dir,
        &caches_dir,
        &[library],
        "1.6.4",
        "x86_64",
        true,
    )
    .await
    .unwrap();

    let natives_dir = natives_root.join("1.6.4");
    assert!(natives_dir.exists());
    assert_eq!(std::fs::read_dir(&natives_dir).unwrap().count(), 0);
}
