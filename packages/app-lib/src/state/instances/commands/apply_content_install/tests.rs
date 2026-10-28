use super::*;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::time::Duration;

#[test]
fn materialization_keys_reject_case_collisions_on_insensitive_platforms() {
    assert_eq!(
        materialization_path_key("mods/Foo.jar")
            == materialization_path_key("mods/foo.jar"),
        cfg!(any(windows, target_os = "macos"))
    );
}

#[test]
fn toggle_refuses_to_replace_an_existing_sibling_file() {
    let directory = tempfile::tempdir().unwrap();
    let mods = directory.path().join("mods");
    std::fs::create_dir_all(&mods).unwrap();
    std::fs::write(mods.join("sibling.jar"), b"enabled").unwrap();
    std::fs::write(mods.join("sibling.jar.disabled"), b"disabled").unwrap();

    let error =
        resolve_toggle_paths(directory.path(), "mods/sibling.jar", Some(false))
            .unwrap_err();
    assert!(error.to_string().contains("already exists"));

    let error = resolve_toggle_paths(
        directory.path(),
        "mods/sibling.jar.disabled",
        Some(true),
    )
    .unwrap_err();
    assert!(error.to_string().contains("already exists"));

    assert_eq!(std::fs::read(mods.join("sibling.jar")).unwrap(), b"enabled");
    assert_eq!(
        std::fs::read(mods.join("sibling.jar.disabled")).unwrap(),
        b"disabled"
    );
}

#[test]
fn toggle_renames_when_the_destination_is_free() {
    let directory = tempfile::tempdir().unwrap();
    let mods = directory.path().join("mods");
    std::fs::create_dir_all(&mods).unwrap();
    std::fs::write(mods.join("single.jar"), b"enabled").unwrap();
    std::fs::write(mods.join("already.jar.disabled"), b"disabled").unwrap();

    let (current, enabled, next) =
        resolve_toggle_paths(directory.path(), "mods/single.jar", Some(false))
            .unwrap();
    assert_eq!(current, "mods/single.jar");
    assert!(!enabled);
    assert_eq!(next, "mods/single.jar.disabled");

    // Disabling a file that is already disabled keeps its current path.
    let (current, enabled, next) = resolve_toggle_paths(
        directory.path(),
        "mods/already.jar.disabled",
        Some(false),
    )
    .unwrap();
    assert_eq!(current, "mods/already.jar.disabled");
    assert!(!enabled);
    assert_eq!(next, "mods/already.jar.disabled");
}

#[tokio::test]
async fn materialized_instances_do_not_share_writable_inodes() {
    let directory = tempfile::tempdir().unwrap();
    let shared = directory.path().join("shared");
    let first = directory.path().join("first/mod.jar");
    let second = directory.path().join("second/mod.jar");
    tokio::fs::write(&shared, b"original").await.unwrap();
    materialize_project_download(&shared, &first).await.unwrap();
    materialize_project_download(&shared, &second)
        .await
        .unwrap();
    tokio::fs::write(&first, b"modified").await.unwrap();
    assert_eq!(tokio::fs::read(shared).await.unwrap(), b"original");
    assert_eq!(tokio::fs::read(second).await.unwrap(), b"original");
}

#[test]
fn resource_pack_target_preferences_ignore_game_version() {
    assert_eq!(
        target_preferences(
            "26.2".to_string(),
            ModLoader::NeoForge,
            ContentType::ResourcePack,
        ),
        ResolutionPreferences {
            game_versions: Vec::new(),
            loaders: vec!["minecraft".to_string()],
        },
    );
}

#[test]
fn backup_relative_path_for_update_naming() {
    assert_eq!(
        backup_relative_path_for_update("mods/Mod-1.0.jar", "mods/Mod-1.1.jar")
            .as_deref(),
        Some("mods/Mod-1.1.jar_Mod-1.0.jar.old")
    );
    assert_eq!(
        backup_relative_path_for_update(
            "mods/Mod-1.0.jar.disabled",
            "mods/Mod-1.1.jar.disabled"
        )
        .as_deref(),
        Some("mods/Mod-1.1.jar_Mod-1.0.jar.old")
    );
    assert_eq!(
        backup_relative_path_for_update(
            "schematics/del/Old.litematic",
            "schematics/del/New.litematic"
        )
        .as_deref(),
        Some("schematics/del/New.litematic_Old.litematic.old")
    );
    assert_eq!(
        backup_relative_path_for_update("mods/Mod.jar", "mods/Mod.jar"),
        None
    );
}

fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write as _;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        for (name, bytes) in entries {
            zip.start_file(*name, zip::write::FileOptions::<()>::default())
                .expect("start entry");
            zip.write_all(bytes).expect("write entry");
        }
        zip.finish().expect("finish zip");
    }
    buf.into_inner()
}

#[test]
fn wrapped_resource_pack_zip_plans_whole() {
    let bytes = zip_bytes(&[
        ("My Pack/pack.mcmeta", br#"{"pack":{"pack_format":15}}"#),
        ("My Pack/assets/minecraft/lang/en_us.json", b"{}"),
        ("My Pack/", b""),
    ]);
    assert!(
        wrapped_pack_plan(&bytes, ProjectType::ResourcePack)
            == Some(WrappedPackPlan::Whole {
                wrapper: "My Pack".to_string()
            })
    );
}

#[test]
fn wrapped_shader_pack_zip_plans_whole() {
    let bytes = zip_bytes(&[
        ("My Shader/shaders/a.fsh", b"v"),
        ("My Shader/shaders/b.vsh", b"v"),
    ]);
    assert!(
        wrapped_pack_plan(&bytes, ProjectType::ShaderPack)
            == Some(WrappedPackPlan::Whole {
                wrapper: "My Shader".to_string()
            })
    );
}

#[test]
fn flat_or_other_archives_have_no_plan() {
    let flat_rp = zip_bytes(&[
        ("pack.mcmeta", br#"{"pack":{"pack_format":15}}"#),
        ("assets/x.txt", b"x"),
    ]);
    assert_eq!(wrapped_pack_plan(&flat_rp, ProjectType::ResourcePack), None);
    let mod_zip = zip_bytes(&[("fabric.mod.json", b"{}")]);
    assert_eq!(wrapped_pack_plan(&mod_zip, ProjectType::Mod), None);
    let mixed = zip_bytes(&[
        ("My Pack/pack.mcmeta", br#"{"pack":{"pack_format":15}}"#),
        ("loose.txt", b"x"),
    ]);
    assert_eq!(wrapped_pack_plan(&mixed, ProjectType::ResourcePack), None);
    let no_marker = zip_bytes(&[("My Pack/assets/x.txt", b"x")]);
    assert_eq!(
        wrapped_pack_plan(&no_marker, ProjectType::ResourcePack),
        None
    );
}

#[test]
fn wrapper_with_pack_children_plans_each_child() {
    let bytes = zip_bytes(&[
        ("packs/A/pack.mcmeta", br#"{"pack":{"pack_format":15}}"#),
        ("packs/A/assets/a.txt", b"x"),
        ("packs/B/pack.mcmeta", br#"{"pack":{"pack_format":15}}"#),
        ("packs/B/assets/b.txt", b"x"),
    ]);
    assert!(
        wrapped_pack_plan(&bytes, ProjectType::ResourcePack)
            == Some(WrappedPackPlan::Children {
                children: vec!["A".to_string(), "B".to_string()]
            })
    );
}

#[test]
fn shaderpack_collection_with_direct_shaders_plans_whole() {
    // A zipped shaderpacks folder that itself contains a pack folder
    // named "shaders" plus other packs: the wrapper is treated as one
    // pack (its marker sits at the wrapper root).
    let bytes = zip_bytes(&[
        ("shaderpacks/shaders/a.fsh", b"v"),
        ("shaderpacks/Other Pack/shaders/b.vsh", b"v"),
        ("shaderpacks/Another.zip", b"zip"),
    ]);
    assert!(
        wrapped_pack_plan(&bytes, ProjectType::ShaderPack)
            == Some(WrappedPackPlan::Whole {
                wrapper: "shaderpacks".to_string()
            })
    );
}

#[tokio::test]
async fn content_writes_wait_for_the_existing_sqlite_writer() {
    let directory = tempfile::tempdir().unwrap();
    let options = SqliteConnectOptions::new()
        .filename(directory.path().join("content-write.db"))
        .create_if_missing(true)
        .busy_timeout(Duration::from_secs(2));
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();

    let first = begin_content_write(&pool).await.unwrap();
    let second_pool = pool.clone();
    let second =
        tokio::spawn(async move { begin_content_write(&second_pool).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(!second.is_finished());
    first.commit().await.unwrap();

    let second = tokio::time::timeout(Duration::from_secs(2), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    second.rollback().await.unwrap();
}

#[test]
fn update_filename_replaces_the_official_name_inside_custom_names() {
    assert_eq!(
        preserved_update_relative_path(
            "mods/銆愪笘鐣岀敓鎴愩€慣erralith_v2.5.1.jar",
            Some("Terralith_v2.5.1.jar"),
            "Terralith_v2.6.1.jar",
        )
        .unwrap(),
        "mods/銆愪笘鐣岀敓鎴愩€慣erralith_v2.6.1.jar"
    );
    assert_eq!(
        preserved_update_relative_path(
            "mods/prefix-old.jar-suffix.disabled",
            Some("old.jar"),
            "new.jar",
        )
        .unwrap(),
        "mods/prefix-new.jar-suffix.disabled"
    );
}

#[test]
fn update_filename_preserves_unmatched_or_ambiguous_custom_names() {
    assert_eq!(
        preserved_update_relative_path(
            "mods/my-custom-name.jar",
            Some("official-old.jar"),
            "official-new.jar",
        )
        .unwrap(),
        "mods/my-custom-name.jar"
    );
    assert_eq!(
        preserved_update_relative_path(
            "mods/old.jar-old.jar",
            Some("old.jar"),
            "new.jar",
        )
        .unwrap(),
        "mods/old.jar-old.jar"
    );
}
