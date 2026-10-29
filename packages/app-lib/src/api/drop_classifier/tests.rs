use super::*;
use std::io::Write;
use tempfile::tempdir;

/// Minimal valid world `level.dat` payload: NBT root tag `0x0A` with
/// root name `Data` followed by an empty compound.
const VALID_LEVEL_DAT: &[u8] = b"\x0A\x00\x04Data\x00";

/// Minimal valid `pack.mcmeta` with a `pack_format` value.
const VALID_PACK_MCMETA: &[u8] =
    br#"{"pack":{"pack_format":15,"description":"test"}}"#;

#[test]
fn test_nonexistent_path() {
    let result = classify_dropped_item(Path::new("/nonexistent/path"));
    assert!(
        matches!(result, DroppedItemType::Unknown { .. }),
        "nonexistent path should be Unknown"
    );
}

#[test]
fn cancelled_classification_stops_before_touching_input() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let result = classify_dropped_item_with_candidates_cancellable(
        Path::new("/nonexistent/path"),
        false,
        &cancellation,
    );
    assert!(matches!(
        result,
        DroppedItemType::Unknown { reason }
            if reason == "Operation cancelled"
    ));
}

#[test]
fn cancelled_zip_extraction_stops_before_opening_archive() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let result = extract_zip_to_dir_cancellable(
        Path::new("/nonexistent/archive.zip"),
        Path::new("/nonexistent/output"),
        &cancellation,
    );
    assert_eq!(result, Err("Operation cancelled".to_string()));
}

#[test]
fn test_regular_mod_jar() {
    let dir = tempdir().expect("temp dir");
    let jar_path = dir.path().join("testmod.jar");

    // Create a minimal ZIP with a fabric.mod.json.
    let file = std::fs::File::create(&jar_path).expect("create jar");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("fabric.mod.json", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(b"{}").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&jar_path);
    assert!(
        matches!(result, DroppedItemType::Mod { .. }),
        "jar with fabric mod should be classified as Mod: {result:?}"
    );
}

#[test]
fn test_litematic_file() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("build.litematic");
    std::fs::write(&path, "fake litematic data").expect("write");

    let result = classify_dropped_item(&path);
    assert!(
        matches!(result, DroppedItemType::Litematic { .. }),
        "litematic file should be classified as Litematic"
    );
}

#[test]
fn test_resource_pack_folder() {
    let dir = tempdir().expect("temp dir");
    let rp = dir.path().join("my_resource_pack");
    std::fs::create_dir(&rp).expect("create dir");
    std::fs::write(rp.join("pack.mcmeta"), VALID_PACK_MCMETA)
        .expect("write pack.mcmeta");
    std::fs::create_dir(rp.join("assets")).expect("create assets dir");

    let result = classify_dropped_item(&rp);
    assert!(
        matches!(result, DroppedItemType::ResourcePack { .. }),
        "folder with pack.mcmeta should be ResourcePack"
    );
}

#[test]
fn test_world_save() {
    let dir = tempdir().expect("temp dir");
    let world = dir.path().join("New World");
    std::fs::create_dir(&world).expect("create dir");
    std::fs::write(world.join("level.dat"), VALID_LEVEL_DAT)
        .expect("write level.dat");

    let result = classify_dropped_item(&world);
    assert!(
        matches!(result, DroppedItemType::WorldSave { .. }),
        "folder with level.dat should be WorldSave"
    );
}

#[test]
fn test_multimc_launcher_folder() {
    let dir = tempdir().expect("temp dir");
    std::fs::write(dir.path().join("multimc.cfg"), "").expect("write");

    let result = classify_dropped_item(dir.path());
    assert!(
        matches!(result, DroppedItemType::Launcher { launcher_type, .. } if launcher_type == ImportLauncherType::MultiMC),
        "folder with multimc.cfg should be MultiMC launcher"
    );
}

#[test]
fn test_shader_pack_folder() {
    let dir = tempdir().expect("temp dir");
    let shaders = dir.path().join("shaders");
    std::fs::create_dir(&shaders).expect("create shaders dir");
    std::fs::write(shaders.join("composite.fsh"), "shader")
        .expect("write shader file");

    let result = classify_dropped_item(dir.path());
    assert!(
        matches!(result, DroppedItemType::ShaderPack { .. }),
        "folder with shaders/ should be ShaderPack"
    );
}

#[test]
fn test_unknown_file() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("random.xyz");
    std::fs::write(&path, "data").expect("write");

    let result = classify_dropped_item(&path);
    assert!(
        matches!(result, DroppedItemType::Unknown { .. }),
        "unknown extension should be Unknown"
    );
}

#[test]
fn test_zip_single_file() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("test.zip");

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("test.txt", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(b"hello").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    // Single .txt file inside ZIP → after extraction, classify_file sees .txt → Unknown.
    assert!(
        matches!(result, DroppedItemType::Unknown { .. }),
        "zip with single .txt should resolve to Unknown: {result:?}"
    );
}

#[test]
fn test_zip_with_mod_jar() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("modpack.zip");

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);

    // Create a JAR at the archive root.
    zip.start_file("testmod.jar", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    // The extracted .jar has no readable manifest, so classify_jar falls
    // back to Mod (the default for JAR files).
    zip.write_all(b"fake jar content").expect("write");
    zip.finish().expect("finish");

    let result = classify_zip_with_extraction(&zip_path);
    // Force-analysis extracts the single item and classifies it as a Mod.
    assert!(
        matches!(result, DroppedItemType::Mod { .. }),
        "zip with a single testmod.jar should be classified as Mod: {result:?}"
    );
}

#[test]
fn test_zip_nested_world_save() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("world.zip");

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    // Entries under a single shared root folder, as produced by zipping
    // the world folder itself.
    zip.start_file(
        "My World/level.dat",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(VALID_LEVEL_DAT).expect("write");
    zip.start_file(
        "My World/region/r.0.0.mca",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(b"mca").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::WorldSave { .. }),
        "zip with a nested level.dat should be classified as WorldSave: {result:?}"
    );
}

#[test]
fn test_zip_nested_resource_pack() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("pack.zip");

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "My Pack/pack.mcmeta",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(VALID_PACK_MCMETA).expect("write");
    zip.start_file("My Pack/assets/", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::ResourcePack { .. }),
        "zip with a nested pack.mcmeta should be classified as ResourcePack: {result:?}"
    );
}

#[test]
fn test_zip_modpack_with_nested_level_dat_stays_modpack() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("pack.mrpack");

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "modrinth.index.json",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(b"{\"formatVersion\":1,\"game\":\"minecraft\",\"versionId\":\"1\",\"name\":\"p\",\"files\":[],\"dependencies\":{\"minecraft\":\"1.20.1\"}}")
            .expect("write");
    // A modpack may ship a world in overrides; the pack signature must
    // win over the level.dat marker.
    zip.start_file(
        "overrides/world/level.dat",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(b"fake").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::Modpack { .. }),
        "mrpack with nested level.dat should still be Modpack: {result:?}"
    );
}

#[test]
fn test_zip_compressed_minecraft_is_instance_not_resource_pack() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("minecraft.zip");

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    // A .minecraft folder embeds resource packs, shader packs and worlds;
    // the instance signature must win over those markers.
    for name in [
        ".minecraft/versions/1.20.1/1.20.1.json",
        ".minecraft/resourcepacks/My Pack/pack.mcmeta",
        ".minecraft/saves/New World/level.dat",
    ] {
        zip.start_file(name, zip::write::FileOptions::<()>::default())
            .expect("start entry");
        zip.write_all(b"x").expect("write");
    }
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(
            result,
            DroppedItemType::Launcher {
                launcher_type: ImportLauncherType::Generic,
                ..
            }
        ),
        "compressed .minecraft should classify as a Generic instance: {result:?}"
    );
}

#[test]
fn test_zip_launcher_folder_with_multiple_instances() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("launcher.zip");

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    for name in [
        "multimc.cfg",
        "instances/Alpha/instance.cfg",
        "instances/Beta/instance.cfg",
    ] {
        zip.start_file(name, zip::write::FileOptions::<()>::default())
            .expect("start entry");
        zip.write_all(b"x").expect("write");
    }
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(
            result,
            DroppedItemType::Launcher {
                launcher_type: ImportLauncherType::MultiMC,
                ..
            }
        ),
        "launcher folder zip should classify as MultiMC: {result:?}"
    );
}

#[test]
fn test_zip_wrapping_folder_modpack() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("pack.zip");

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "My Pack/modrinth.index.json",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(
            b"{\"formatVersion\":1,\"game\":\"minecraft\",\"versionId\":\"1\",\"name\":\"p\",\"files\":[],\"dependencies\":{\"minecraft\":\"1.20.1\"}}",
        )
        .expect("write");
    zip.start_file(
        "My Pack/overrides/config/x.txt",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(b"x").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::Modpack { .. }),
        "modpack inside a wrapping folder should classify as Modpack: {result:?}"
    );
}

#[test]
fn test_zip_deeply_nested_world_save() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("backup.zip");

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "backup/worlds/My World/level.dat",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(VALID_LEVEL_DAT).expect("write");
    zip.start_file(
        "backup/worlds/My World/region/r.0.0.mca",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(b"x").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::WorldSave { .. }),
        "world nested under folders should classify as WorldSave: {result:?}"
    );
}

#[test]
fn test_zip_nested_instance_within_limit() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("nested.zip");

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "d0/d1/d2/d3/d4/instance.cfg",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(b"x").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(
            result,
            DroppedItemType::Launcher {
                launcher_type: ImportLauncherType::MultiMC,
                ..
            }
        ),
        "instance at 5 folder levels should classify as MultiMC: {result:?}"
    );
}

#[test]
fn test_zip_nesting_depth_limit() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("deep.zip");

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "d0/d1/d2/d3/d4/d5/instance.cfg",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(b"x").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::Unknown { .. }),
        "nesting beyond MAX_ZIP_NESTING_DEPTH should stay Unknown: {result:?}"
    );
}

#[test]
fn test_extract_all_rejects_path_traversal() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("evil.zip");

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    // A malicious entry that tries to escape the extraction directory.
    zip.start_file("../../evil.txt", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(b"pwned").expect("write");
    zip.finish().expect("finish");

    let out_dir = tempdir().expect("temp dir");
    let Ok(mut archive) =
        zip::ZipArchive::new(std::fs::File::open(&zip_path).expect("open zip"))
    else {
        panic!("zip should open");
    };
    extract_all(&mut archive, out_dir.path()).expect("safe ZIP should extract");

    assert!(
        !out_dir.path().join("evil.txt").exists()
            && !out_dir.path().join("..").join("evil.txt").exists(),
        "path traversal entry must not be extracted outside the target dir"
    );
}

// ── Golden tests: real launcher / pack packaging habits ──────────────

#[test]
fn test_zip_curseforge_pack() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("cf.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("manifest.json", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(
            br#"{"minecraft":{"version":"1.20.1"},"manifestType":"minecraftModpack","manifestVersion":1,"files":[],"overrides":"overrides"}"#,
        )
        .expect("write");
    zip.start_file("mods/a.jar", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(b"jar").expect("write");
    zip.start_file(
        "overrides/config/x.toml",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(b"x").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::Modpack { .. }),
        "CurseForge pack should classify as Modpack: {result:?}"
    );
}

#[test]
fn test_zip_mcbbs_pack() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("mcbbs.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("manifest.json", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(br#"{"addons":[{"id":291}]}"#).expect("write");
    zip.start_file("mcbbs.packmeta", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(b"{}").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::Modpack { .. }),
        "MCBBS pack should classify as Modpack: {result:?}"
    );
}

#[test]
fn test_zip_mmc_export_pack() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("mmc.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("mmc-pack.json", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(b"{}").expect("write");
    // instance.cfg would mark a launcher instance; the MMC export
    // manifest must win because a pack can carry instance metadata.
    zip.start_file("instance.cfg", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(b"").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::Modpack { .. }),
        "MMC export should classify as Modpack: {result:?}"
    );
}

#[test]
fn test_zip_hmcl_pack() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("hmcl.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("modpack.json", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(b"{}").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::Modpack { .. }),
        "HMCL pack should classify as Modpack: {result:?}"
    );
}

#[test]
fn test_zip_launcher_bundle() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("bundle.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    // A launcher bundle wraps another pack inside modpack.zip.
    zip.start_file("modpack.zip", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(b"inner pack bytes").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::Modpack { .. }),
        "launcher bundle should classify as Modpack: {result:?}"
    );
}

#[test]
fn test_zip_pcl_single_instance_folder() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("pcl.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    for name in [
        "我的整合包/.minecraft/versions/1.20.1/1.20.1.json",
        "我的整合包/.minecraft/mods/a.jar",
        "我的整合包/launcher_profiles.json",
    ] {
        zip.start_file(name, zip::write::FileOptions::<()>::default())
            .expect("start entry");
        zip.write_all(b"x").expect("write");
    }
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(
            result,
            DroppedItemType::Launcher {
                launcher_type: ImportLauncherType::Generic,
                ..
            }
        ),
        "PCL-style single instance folder should classify as Generic: {result:?}"
    );
}

#[test]
fn test_zip_hmcl_portable_instance() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("hmcl.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    for name in [
        "HMCL/.hmcl/config/launcher-settings.json",
        "HMCL/.minecraft/versions/1.20.1/1.20.1.json",
        "HMCL/hmcl.json",
    ] {
        zip.start_file(name, zip::write::FileOptions::<()>::default())
            .expect("start entry");
        zip.write_all(b"{}").expect("write");
    }
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(
            result,
            DroppedItemType::Launcher {
                launcher_type: ImportLauncherType::HMCL,
                ..
            }
        ),
        "HMCL portable instance should classify as HMCL: {result:?}"
    );
}

#[test]
fn test_zip_nested_world_zips() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("backup.zip");

    let mut inner = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut inner));
        writer
            .start_file(
                "My World/level.dat",
                zip::write::FileOptions::<()>::default(),
            )
            .expect("start inner entry");
        writer.write_all(VALID_LEVEL_DAT).expect("write inner");
        writer.finish().expect("finish inner");
    }

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    for name in ["Backup/worlds/world1.zip", "Backup/worlds/world2.zip"] {
        zip.start_file(name, zip::write::FileOptions::<()>::default())
            .expect("start entry");
        zip.write_all(&inner).expect("write");
    }
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::WorldSave { .. }),
        "backup of world zips should classify as WorldSave: {result:?}"
    );
}

#[test]
fn test_zip_nested_mrpack() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("downloads.zip");

    let mut inner = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut inner));
        writer
            .start_file(
                "modrinth.index.json",
                zip::write::FileOptions::<()>::default(),
            )
            .expect("start inner entry");
        writer
                .write_all(
                    br#"{"formatVersion":1,"files":[],"dependencies":{"minecraft":"1.20.1"}}"#,
                )
                .expect("write inner");
        writer.finish().expect("finish inner");
    }

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "Downloads/packs/mypack.mrpack",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(&inner).expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::Modpack { .. }),
        "nested mrpack should classify as Modpack: {result:?}"
    );
}

#[test]
fn test_zip_nested_zip_chain_within_limit() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("chain.zip");

    let mut world = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut world));
        writer
            .start_file(
                "My World/level.dat",
                zip::write::FileOptions::<()>::default(),
            )
            .expect("start world entry");
        writer.write_all(VALID_LEVEL_DAT).expect("write world");
        writer.finish().expect("finish world");
    }
    let mut c = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut c));
        writer
            .start_file("world.zip", zip::write::FileOptions::<()>::default())
            .expect("start c entry");
        writer.write_all(&world).expect("write c");
        writer.finish().expect("finish c");
    }
    let mut b = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut b));
        writer
            .start_file("c.zip", zip::write::FileOptions::<()>::default())
            .expect("start b entry");
        writer.write_all(&c).expect("write b");
        writer.finish().expect("finish b");
    }
    let mut a = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut a));
        writer
            .start_file("b.zip", zip::write::FileOptions::<()>::default())
            .expect("start a entry");
        writer.write_all(&b).expect("write a");
        writer.finish().expect("finish a");
    }

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("a.zip", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(&a).expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::WorldSave { .. }),
        "4-level nested zip chain should classify as WorldSave: {result:?}"
    );
}

#[test]
fn test_zip_mixed_backup_prefers_first_hit() {
    // A backup containing an instance, a world and a resource pack is
    // classified by the first (alphabetical) hit — instances/ wins.
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("mixed.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    for name in [
        "Backup/instances/Alpha/.minecraft/versions/1.20.1/1.20.1.json",
        "Backup/worlds/My World/level.dat",
        "Backup/resourcepacks/My Pack/pack.mcmeta",
    ] {
        zip.start_file(name, zip::write::FileOptions::<()>::default())
            .expect("start entry");
        zip.write_all(b"x").expect("write");
    }
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(
            result,
            DroppedItemType::Launcher {
                launcher_type: ImportLauncherType::Generic,
                ..
            }
        ),
        "mixed backup should classify as the first hit (Generic instance): {result:?}"
    );
}

#[test]
fn test_zip_instance_wins_over_embedded_content() {
    // The instance layer must beat every content marker embedded inside
    // the game folder.
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("backup.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    for name in [
        "我的存档备份/.minecraft/versions/1.20.1/1.20.1.json",
        "我的存档备份/.minecraft/resourcepacks/Vanilla Tweaks/pack.mcmeta",
        "我的存档备份/.minecraft/saves/New World/level.dat",
        "我的存档备份/.minecraft/shaderpacks/My Shader/shaders/a.fsh",
    ] {
        zip.start_file(name, zip::write::FileOptions::<()>::default())
            .expect("start entry");
        zip.write_all(b"x").expect("write");
    }
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(
            result,
            DroppedItemType::Launcher {
                launcher_type: ImportLauncherType::Generic,
                ..
            }
        ),
        "instance must win over embedded content markers: {result:?}"
    );
}

// ── Hardening: content confirmation, noise pruning, safety ───────────

#[test]
fn test_zip_backslash_world_is_normalized() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("bs.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "My World\\level.dat",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(VALID_LEVEL_DAT).expect("write");
    zip.start_file(
        "My World\\region\\r.0.0.mca",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(b"m").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::WorldSave { .. }),
        "backslash world zip should classify as WorldSave: {result:?}"
    );
}

#[test]
fn test_zip_invalid_pack_mcmeta_is_still_resource_pack() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("fake.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "Fake Pack/pack.mcmeta",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(b"not json").expect("write");
    zip.start_file(
        "Fake Pack/assets/",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::ResourcePack { .. }),
        "classification is name-based, so any pack.mcmeta counts: {result:?}"
    );
}

#[test]
fn test_zip_empty_shaders_is_not_shader_pack() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("fake.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "Fake Shader/shaders/",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(b"").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::Unknown { .. }),
        "empty shaders/ must not classify as ShaderPack: {result:?}"
    );
}

#[test]
fn test_zip_noise_entries_are_skipped() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("noise.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "My World/level.dat",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(VALID_LEVEL_DAT).expect("write");
    for name in [
        "__MACOSX/My World/._level.dat",
        ".git/config",
        "Thumbs.db",
        ".DS_Store",
    ] {
        zip.start_file(name, zip::write::FileOptions::<()>::default())
            .expect("start entry");
        zip.write_all(b"x").expect("write");
    }
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::WorldSave { .. }),
        "noise entries must be skipped during recursion: {result:?}"
    );
}

#[test]
fn test_zip_slip_and_absolute_entries_are_ignored() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("evil.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    for name in ["../../evil.txt", "C:/evil.txt", "/evil.txt"] {
        zip.start_file(name, zip::write::FileOptions::<()>::default())
            .expect("start entry");
        zip.write_all(b"e").expect("write");
    }
    zip.start_file(
        "My World/level.dat",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(VALID_LEVEL_DAT).expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::WorldSave { .. }),
        "unsafe entries must not derail classification: {result:?}"
    );
}

#[test]
fn test_zip_empty_archive_is_unknown() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("empty.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    zip::ZipWriter::new(file).finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::Unknown { .. }),
        "empty zip should be Unknown: {result:?}"
    );
}

#[test]
fn test_zip_dirs_only_archive_is_unknown() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("dirs.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("dir/", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(b"").expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::Unknown { .. }),
        "directories-only zip should be Unknown: {result:?}"
    );
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, entry) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *entry = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc = table[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// Writes a minimal ZIP with one legacy-encrypted (general-purpose bit 0)
/// stored entry plus a plain `level.dat`. The zip crate does not need the
/// `aes-crypto` feature to read names of encrypted entries, which is all
/// classification touches.
fn write_encrypted_entry_zip(path: &Path) {
    fn u16(out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&v.to_le_bytes());
    }

    let entries: [(&[u8], &[u8], bool); 2] = [
        (b"secret.bin", b"encrypted payload", true),
        (b"level.dat", VALID_LEVEL_DAT, false),
    ];
    let mut out = Vec::new();
    let mut offsets = Vec::new();
    for (name, data, encrypted) in entries {
        offsets.push(out.len() as u32);
        let flags: u16 = u16::from(encrypted);
        let data_size = data.len() as u32 + if encrypted { 12 } else { 0 };
        out.extend_from_slice(b"PK\x03\x04");
        u16(&mut out, 20);
        u16(&mut out, flags);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u32(&mut out, crc32(data));
        u32(&mut out, data_size);
        u32(&mut out, data.len() as u32);
        u16(&mut out, name.len() as u16);
        u16(&mut out, 0);
        out.extend_from_slice(name);
        if encrypted {
            out.extend_from_slice(&[0u8; 12]);
        }
        out.extend_from_slice(data);
    }

    let cd_start = out.len() as u32;
    for (i, (name, data, encrypted)) in entries.into_iter().enumerate() {
        let flags: u16 = u16::from(encrypted);
        let data_size = data.len() as u32 + if encrypted { 12 } else { 0 };
        out.extend_from_slice(b"PK\x01\x02");
        u16(&mut out, 20);
        u16(&mut out, 20);
        u16(&mut out, flags);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u32(&mut out, crc32(data));
        u32(&mut out, data_size);
        u32(&mut out, data.len() as u32);
        u16(&mut out, name.len() as u16);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u32(&mut out, 0);
        u32(&mut out, offsets[i]);
        out.extend_from_slice(name);
    }
    let cd_size = out.len() as u32 - cd_start;

    out.extend_from_slice(b"PK\x05\x06");
    u16(&mut out, 0);
    u16(&mut out, 0);
    u16(&mut out, entries.len() as u16);
    u16(&mut out, entries.len() as u16);
    u32(&mut out, cd_size);
    u32(&mut out, cd_start);
    u16(&mut out, 0);

    std::fs::write(path, out).expect("write encrypted zip");
}

#[test]
fn test_zip_encrypted_entry_does_not_break_classification() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("enc.zip");
    write_encrypted_entry_zip(&zip_path);

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::WorldSave { .. }),
        "encrypted sibling entry must not break world classification: {result:?}"
    );
}

#[test]
fn test_disabled_jar_and_zip_files() {
    let dir = tempdir().expect("temp dir");
    let jar = dir.path().join("mod.jar.disabled");
    std::fs::write(&jar, b"not a real jar").expect("write");

    let zip_path = dir.path().join("pack.zip.disabled");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("level.dat", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(VALID_LEVEL_DAT).expect("write");
    zip.finish().expect("finish");

    let jar_result = classify_dropped_item(&jar);
    assert!(
        matches!(jar_result, DroppedItemType::Mod { .. }),
        "mod.jar.disabled should classify as Mod: {jar_result:?}"
    );
    let zip_result = classify_dropped_item(&zip_path);
    assert!(
        matches!(zip_result, DroppedItemType::WorldSave { .. }),
        "pack.zip.disabled should classify as WorldSave: {zip_result:?}"
    );
}

#[test]
fn test_nested_zip_staging_disk_space_guard() {
    let dir = tempdir().expect("temp dir");
    // Zero bytes are always available; no filesystem has u64::MAX free.
    assert!(temp_dir_has_space(dir.path(), 0));
    assert!(!temp_dir_has_space(dir.path(), u64::MAX));
}

#[test]
fn nested_zip_requires_unpack_confirmation_first() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("backup.zip");

    let mut inner = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut inner));
        writer
            .start_file(
                "My World/level.dat",
                zip::write::FileOptions::<()>::default(),
            )
            .expect("start inner entry");
        writer.write_all(VALID_LEVEL_DAT).expect("write inner");
        writer.finish().expect("finish inner");
    }

    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "Backup/worlds/world1.zip",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(&inner).expect("write");
    zip.finish().expect("finish");

    let pending = classify_dropped_item_without_nested_unpack(&zip_path);
    assert!(
        matches!(
            &pending,
            DroppedItemType::Unknown { reason }
                if reason.contains("nested archives")
                    && reason.contains("bytes")
        ),
        "first pass must report the nested archive without unpacking: {pending:?}"
    );

    let full = classify_dropped_item(&zip_path);
    assert!(
        matches!(full, DroppedItemType::WorldSave { .. }),
        "confirmed pass may unpack nested archives: {full:?}"
    );
}

#[test]
fn flat_content_does_not_need_nested_unpack_confirmation() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("world.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("level.dat", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(VALID_LEVEL_DAT).expect("write");
    zip.finish().expect("finish");

    let result = classify_dropped_item_without_nested_unpack(&zip_path);
    assert!(
        matches!(result, DroppedItemType::WorldSave { .. }),
        "flat archives need no nested-unpack confirmation: {result:?}"
    );
}

#[test]
fn test_pack_mcmeta_with_bom_is_valid() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("bom.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("pack.mcmeta", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(b"\xEF\xBB\xBF{\"pack\":{\"pack_format\":16}}")
        .expect("write");
    zip.start_file("assets/", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(result, DroppedItemType::ResourcePack { .. }),
        "pack.mcmeta with a UTF-8 BOM should classify as ResourcePack: {result:?}"
    );
}

/// Writes a minimal ZIP with raw (possibly non-UTF-8) entry names and no
/// UTF-8 flag, as produced by some Chinese packaging tools.
fn write_raw_name_zip(path: &Path, entries: &[(&[u8], &[u8])]) {
    fn u16(out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&v.to_le_bytes());
    }

    let mut out = Vec::new();
    let mut offsets = Vec::new();
    for (name, data) in entries {
        offsets.push(out.len() as u32);
        out.extend_from_slice(b"PK\x03\x04");
        u16(&mut out, 20);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u32(&mut out, crc32(data));
        u32(&mut out, data.len() as u32);
        u32(&mut out, data.len() as u32);
        u16(&mut out, name.len() as u16);
        u16(&mut out, 0);
        out.extend_from_slice(name);
        out.extend_from_slice(data);
    }

    let cd_start = out.len() as u32;
    for (i, (name, data)) in entries.iter().enumerate() {
        out.extend_from_slice(b"PK\x01\x02");
        u16(&mut out, 20);
        u16(&mut out, 20);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u32(&mut out, crc32(data));
        u32(&mut out, data.len() as u32);
        u32(&mut out, data.len() as u32);
        u16(&mut out, name.len() as u16);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u16(&mut out, 0);
        u32(&mut out, 0);
        u32(&mut out, offsets[i]);
        out.extend_from_slice(name);
    }
    let cd_size = out.len() as u32 - cd_start;

    out.extend_from_slice(b"PK\x05\x06");
    u16(&mut out, 0);
    u16(&mut out, 0);
    u16(&mut out, entries.len() as u16);
    u16(&mut out, entries.len() as u16);
    u32(&mut out, cd_size);
    u32(&mut out, cd_start);
    u16(&mut out, 0);

    std::fs::write(path, out).expect("write raw-name zip");
}

#[test]
fn extract_zip_to_dir_handles_non_utf8_names() {
    // Chinese tools store names as GB18030 without the UTF-8 flag; the
    // zip crate's by_name lookup cannot match them, so extraction must
    // read entries by index.
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("gbk.zip");
    let (gbk_name, _, _) = encoding_rs::GB18030.encode("我的世界/level.dat");
    write_raw_name_zip(&zip_path, &[(gbk_name.as_ref(), VALID_LEVEL_DAT)]);

    let out_dir = tempdir().expect("temp out dir");
    extract_zip_to_dir(&zip_path, out_dir.path()).expect("extract GB18030 zip");
    assert!(out_dir.path().join("我的世界/level.dat").exists());
}

#[test]
fn extract_zip_to_dir_normalizes_backslash_names() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("bs.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "My World\\level.dat",
        zip::write::FileOptions::<()>::default(),
    )
    .expect("start entry");
    zip.write_all(VALID_LEVEL_DAT).expect("write");
    zip.finish().expect("finish");

    let out_dir = tempdir().expect("temp out dir");
    extract_zip_to_dir(&zip_path, out_dir.path())
        .expect("extract backslash zip");
    assert!(out_dir.path().join("My World/level.dat").exists());
}

#[test]
fn extract_zip_to_dir_skips_unsafe_entries_and_rejects_encrypted_entries() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("mixed.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("../../evil.txt", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(b"e").expect("write");
    zip.start_file("level.dat", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(VALID_LEVEL_DAT).expect("write");
    zip.finish().expect("finish");

    let out_dir = tempdir().expect("temp out dir");
    extract_zip_to_dir(&zip_path, out_dir.path()).expect("extract mixed zip");
    assert!(out_dir.path().join("level.dat").exists());
    assert!(!out_dir.path().join("evil.txt").exists());

    let enc_dir = tempdir().expect("temp dir");
    let enc_path = enc_dir.path().join("enc.zip");
    write_encrypted_entry_zip(&enc_path);
    let enc_out = tempdir().expect("temp out dir");
    let error = extract_zip_to_dir(&enc_path, enc_out.path())
        .expect_err("encrypted entries must fail extraction");
    assert!(error.contains("secret.bin"));
    assert!(
        enc_out.path().join("level.dat").exists(),
        "plain entries survive"
    );
    assert!(
        !enc_out.path().join("secret.bin").exists(),
        "encrypted entries are skipped"
    );
}

#[test]
fn extract_zip_to_dir_rejects_archive_with_only_unsafe_entries() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("unsafe-only.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("../../evil.txt", zip::write::FileOptions::<()>::default())
        .expect("start entry");
    zip.write_all(b"e").expect("write");
    zip.finish().expect("finish");

    let out_dir = tempdir().expect("temp out dir");
    let error = extract_zip_to_dir(&zip_path, out_dir.path())
        .expect_err("an archive with no safe entries must fail");
    assert!(error.contains("No ZIP entries could be extracted"));
    assert!(!out_dir.path().join("evil.txt").exists());
}

#[test]
fn test_zip_pcl_multiple_version_folders_report_common_parent() {
    let dir = tempdir().expect("temp dir");
    let zip_path = dir.path().join("pcl.zip");
    let file = std::fs::File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    for name in [
        "test/1.7.10/1.7.10.jar",
        "test/1.7.10/1.7.10.json",
        "test/1.8.9/1.8.9.jar",
        "test/1.8.9/1.8.9.json",
        "test/qqwe.txt",
    ] {
        zip.start_file(name, zip::write::FileOptions::<()>::default())
            .expect("start entry");
        zip.write_all(b"x").expect("write");
    }
    zip.finish().expect("finish");

    let result = classify_dropped_item(&zip_path);
    assert!(
        matches!(
            result,
            DroppedItemType::Launcher {
                launcher_type: ImportLauncherType::Generic,
                inner_base: Some(ref base),
                ..
            } if base.as_str() == "test"
        ),
        "multiple sibling instance folders should report the common parent: {result:?}"
    );
}

#[tokio::test]
async fn generic_scan_enumerates_sibling_instance_folders() {
    let dir = tempdir().expect("temp dir");
    let base = dir.path().join("test");
    std::fs::create_dir_all(base.join("1.7.10")).expect("create");
    std::fs::create_dir_all(base.join("1.8.9")).expect("create");
    std::fs::write(base.join("1.7.10/1.7.10.jar"), b"j").expect("write");
    std::fs::write(base.join("1.7.10/1.7.10.json"), b"{}").expect("write");
    std::fs::write(base.join("1.8.9/1.8.9.jar"), b"j").expect("write");
    std::fs::write(base.join("1.8.9/1.8.9.json"), b"{}").expect("write");
    std::fs::write(base.join("qqwe.txt"), b"x").expect("write");

    let instances = crate::api::pack::import::get_importable_instances(
        ImportLauncherType::Generic,
        base,
    )
    .await
    .expect("scan");
    let names: Vec<String> = instances.iter().map(|i| i.name.clone()).collect();
    assert_eq!(instances.len(), 2, "{instances:?}");
    assert!(names.contains(&"1.7.10".to_string()), "{names:?}");
    assert!(names.contains(&"1.8.9".to_string()), "{names:?}");
}
