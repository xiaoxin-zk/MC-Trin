use super::*;

#[tokio::test]
async fn committed_restore_housekeeping_leaves_unfinished_restores_and_instances_untouched()
 {
    let directory = tempfile::tempdir().unwrap();
    let committed = directory.path().join(format!("{RESTORE_PREFIX}complete"));
    let pending = directory.path().join(format!("{RESTORE_PREFIX}pending"));
    let instance = directory.path().join("normal-instance");
    for root in [&committed, &pending, &instance] {
        tokio::fs::create_dir_all(root).await.unwrap();
        tokio::fs::write(root.join("data"), b"preserved")
            .await
            .unwrap();
    }
    tokio::fs::write(committed.join(RESTORE_COMMITTED), b"")
        .await
        .unwrap();
    tokio::fs::write(instance.join(RESTORE_COMMITTED), b"")
        .await
        .unwrap();
    cleanup_committed_restores(directory.path()).await.unwrap();
    assert!(!committed.exists());
    assert!(pending.join("data").exists());
    assert!(instance.join("data").exists());
}

fn restore_directory(path: &str) -> RestoreEntry {
    RestoreEntry {
        path: path.to_string(),
        kind: EntryKind::Directory,
        object_hash: None,
        size: 0,
        unix_mode: None,
        link_target: None,
        link_is_directory: None,
    }
}

fn restore_file(path: &str, hash: &str) -> RestoreEntry {
    RestoreEntry {
        path: path.to_string(),
        kind: EntryKind::File,
        object_hash: Some(hash.to_string()),
        size: 1,
        unix_mode: None,
        link_target: None,
        link_is_directory: None,
    }
}

#[test]
fn restore_plan_counts_file_changes_and_keeps_minimal_roots() {
    let current = BTreeMap::from([
        ("config".to_string(), restore_directory("config")),
        (
            "config/delete.txt".to_string(),
            restore_file("config/delete.txt", "delete"),
        ),
        (
            "config/modify.txt".to_string(),
            restore_file("config/modify.txt", "old"),
        ),
        (
            "config/same.txt".to_string(),
            restore_file("config/same.txt", "same"),
        ),
    ]);
    let target = BTreeMap::from([
        ("config".to_string(), restore_directory("config")),
        (
            "config/add.txt".to_string(),
            restore_file("config/add.txt", "add"),
        ),
        (
            "config/modify.txt".to_string(),
            restore_file("config/modify.txt", "new"),
        ),
        (
            "config/same.txt".to_string(),
            restore_file("config/same.txt", "same"),
        ),
    ]);

    let plan = compare_restore_entries("snapshot", current, target);

    assert_eq!(plan.preview.added_files, 1);
    assert_eq!(plan.preview.modified_files, 1);
    assert_eq!(plan.preview.deleted_files, 1);
    assert_eq!(
        plan.roots
            .iter()
            .map(|root| root.path.as_str())
            .collect::<Vec<_>>(),
        ["config/add.txt", "config/delete.txt", "config/modify.txt"]
    );
}

#[test]
fn restore_plan_collapses_descendants_when_entry_type_changes() {
    let current = BTreeMap::from([
        ("cache".to_string(), restore_directory("cache")),
        (
            "cache/data.bin".to_string(),
            restore_file("cache/data.bin", "old"),
        ),
    ]);
    let target =
        BTreeMap::from([("cache".to_string(), restore_file("cache", "new"))]);

    let plan = compare_restore_entries("snapshot", current, target);

    assert_eq!(plan.preview.modified_files, 1);
    assert_eq!(plan.preview.deleted_files, 1);
    assert_eq!(plan.roots.len(), 1);
    assert_eq!(plan.roots[0].path, "cache");
    assert!(plan.roots[0].had_current);
}

#[tokio::test]
async fn restore_scan_hashes_files_and_skips_exclusions() {
    let temp = tempfile::tempdir().unwrap();
    tokio::fs::create_dir_all(temp.path().join("config"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(temp.path().join("caches"))
        .await
        .unwrap();
    tokio::fs::write(temp.path().join("config/value.txt"), b"value")
        .await
        .unwrap();
    tokio::fs::write(temp.path().join("caches/value.bin"), b"ignored")
        .await
        .unwrap();

    let entries = scan_restore_entries(
        temp.path(),
        true,
        &[],
        &[BackupExclusion {
            path: "caches".to_string(),
            kind: BackupExclusionKind::Directory,
        }],
        None,
    )
    .await
    .unwrap();

    assert!(entries.contains_key("config"));
    assert!(entries.contains_key("config/value.txt"));
    assert!(!entries.contains_key("caches"));
    assert_eq!(
        entries["config/value.txt"].object_hash.as_deref(),
        Some(
            "cd42404d52ad55ccfa9aca4adc828aa5800ad9d385a0671fbcbf724118320619"
        )
    );
}

#[tokio::test]
async fn incremental_restore_replaces_only_changed_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("instance");
    let operation_root = temp.path().join("operation");
    tokio::fs::create_dir_all(root.join("config"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(operation_root.join("staged/config"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(operation_root.join("rollback"))
        .await
        .unwrap();
    tokio::fs::write(root.join("config/unchanged.txt"), b"same")
        .await
        .unwrap();
    tokio::fs::write(root.join("config/changed.txt"), b"current")
        .await
        .unwrap();
    tokio::fs::write(
        operation_root.join("staged/config/changed.txt"),
        b"backup",
    )
    .await
    .unwrap();

    apply_restore_roots(
        &root,
        &operation_root,
        &[RestoreJournalRoot {
            path: "config/changed.txt".to_string(),
            had_current: true,
        }],
    )
    .await
    .unwrap();

    assert_eq!(
        tokio::fs::read(root.join("config/unchanged.txt"))
            .await
            .unwrap(),
        b"same"
    );
    assert_eq!(
        tokio::fs::read(root.join("config/changed.txt"))
            .await
            .unwrap(),
        b"backup"
    );
    assert_eq!(
        tokio::fs::read(operation_root.join("rollback/config/changed.txt"))
            .await
            .unwrap(),
        b"current"
    );
}

#[tokio::test]
async fn cancellation_is_rejected_after_snapshot_saving_starts() {
    let operation_id = Uuid::new_v4();
    let (cancellation, _cancellable) =
        register_backup_operation(operation_id, "instance");
    assert!(cancel_backup(operation_id).await.unwrap());
    assert!(cancellation.is_cancelled());
    BACKUP_CANCELLATIONS.remove(&operation_id);

    let operation_id = Uuid::new_v4();
    let (cancellation, cancellable) =
        register_backup_operation(operation_id, "instance");
    cancellable.store(false, Ordering::Release);
    assert!(!cancel_backup(operation_id).await.unwrap());
    assert!(!cancellation.is_cancelled());
    BACKUP_CANCELLATIONS.remove(&operation_id);
}

#[test]
fn only_one_operation_can_reserve_an_instance() {
    let instance_id = format!("instance-{}", Uuid::new_v4());
    let first = Uuid::new_v4();
    assert!(reserve_instance_operation(&instance_id, first).is_ok());
    assert!(reserve_instance_operation(&instance_id, Uuid::new_v4()).is_err());
    assert!(has_active_instance_operation(&instance_id));
    ACTIVE_INSTANCE_OPERATIONS.remove(&instance_id);
    assert!(!has_active_instance_operation(&instance_id));
}

#[test]
fn normalizes_and_deduplicates_exclusions() {
    assert_eq!(
        canonicalize_exclusions(vec![
            BackupExclusion {
                path: "saves/world".to_string(),
                kind: BackupExclusionKind::Directory,
            },
            BackupExclusion {
                path: "config\\bigdb.sqlite".to_string(),
                kind: BackupExclusionKind::File,
            },
            BackupExclusion {
                path: "saves".to_string(),
                kind: BackupExclusionKind::Directory,
            },
        ])
        .unwrap(),
        [
            BackupExclusion {
                path: "saves".to_string(),
                kind: BackupExclusionKind::Directory,
            },
            BackupExclusion {
                path: "config/bigdb.sqlite".to_string(),
                kind: BackupExclusionKind::File,
            },
        ]
    );
}

#[test]
fn rejects_paths_outside_the_instance() {
    for path in ["", ".", "..", "../saves", "/saves"] {
        assert!(normalize_relative_directory(path).is_err(), "{path}");
    }
}

#[cfg(windows)]
#[test]
fn rejects_drive_qualified_paths_outside_the_instance() {
    // A drive-qualified path only parses as absolute on Windows; on Unix it
    // is a single file name, so the assertion stays Windows-only.
    assert!(normalize_relative_directory("C:\\saves").is_err());
}

#[tokio::test]
async fn independent_repository_migration_creates_expected_schema() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join(DATABASE_FILE);
    let options = SqliteConnectOptions::new()
        .filename(&database)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    BACKUP_MIGRATOR.run(&pool).await.unwrap();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(tables.contains(&"backup_snapshots".to_string()));
    assert!(tables.contains(&"backup_exclusions".to_string()));
    assert!(tables.contains(&"snapshot_entries".to_string()));
    assert!(tables.contains(&"snapshot_exclusions".to_string()));
    assert!(tables.contains(&"backup_objects".to_string()));
    assert!(tables.contains(&"backup_operations".to_string()));
    assert!(!tables.contains(&"backup_selections".to_string()));
    let operation_columns: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('backup_operations') ORDER BY cid",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(operation_columns.contains(&"cancellable".to_string()));
    assert!(operation_columns.contains(&"cancel_requested".to_string()));
    let foreign_key_errors: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(foreign_key_errors.is_empty());
}

#[tokio::test]
async fn operation_cancellation_migration_preserves_existing_operations() {
    let temp = tempfile::tempdir().unwrap();
    let options = SqliteConnectOptions::new()
        .filename(temp.path().join(DATABASE_FILE))
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../backup_migrations/20260920200000_operations.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO backup_operations
			 (id, operation_type, instance_id, state, created_at, updated_at)
			 VALUES ('operation', 'create', 'instance', 'hashing', 1, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../backup_migrations/20260920210000_operation_cancellation.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();

    let row: (String, i64) = sqlx::query_as(
			"SELECT state, cancel_requested FROM backup_operations WHERE id = 'operation'",
		)
		.fetch_one(&pool)
		.await
		.unwrap();
    assert_eq!(row, ("hashing".to_string(), 0));
    let foreign_key_errors: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(foreign_key_errors.is_empty());
}

#[tokio::test]
async fn repository_move_migration_preserves_operations_and_extends_type() {
    let temp = tempfile::tempdir().unwrap();
    let options = SqliteConnectOptions::new()
        .filename(temp.path().join(DATABASE_FILE))
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../backup_migrations/20260920200000_operations.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../backup_migrations/20260920210000_operation_cancellation.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
            "INSERT INTO backup_operations
             (id, operation_type, instance_id, state, created_at, updated_at)
             VALUES ('create-operation', 'create', 'instance', 'completed', 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
            "../../../../backup_migrations/20260920223000_repository_move_operations.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
    let existing: String = sqlx::query_scalar(
            "SELECT operation_type FROM backup_operations WHERE id = 'create-operation'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(existing, "create");
    sqlx::query(
        "INSERT INTO backup_operations
             (id, operation_type, instance_id, state, created_at, updated_at)
             VALUES ('move-operation', 'repository_move', ?, 'queued', 2, 2)",
    )
    .bind(REPOSITORY_OPERATION_KEY)
    .execute(&pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn restore_preview_migration_preserves_operations_and_result_columns() {
    let temp = tempfile::tempdir().unwrap();
    let options = SqliteConnectOptions::new()
        .filename(temp.path().join(DATABASE_FILE))
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    for migration in [
        include_str!(
            "../../../../backup_migrations/20260920200000_operations.sql"
        ),
        include_str!(
            "../../../../backup_migrations/20260920210000_operation_cancellation.sql"
        ),
        include_str!(
            "../../../../backup_migrations/20260920223000_repository_move_operations.sql"
        ),
    ] {
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    }
    sqlx::query(
        "INSERT INTO backup_operations
             (id, operation_type, instance_id, state, created_at, updated_at)
             VALUES ('existing', 'restore', 'instance', 'completed', 1, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
            "../../../../backup_migrations/20260920230000_restore_preview_operations.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
    let existing: String = sqlx::query_scalar(
        "SELECT operation_type FROM backup_operations WHERE id = 'existing'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(existing, "restore");
    sqlx::query(
            "INSERT INTO backup_operations
             (id, operation_type, instance_id, state, created_at, updated_at,
              preview_added_files, preview_modified_files, preview_deleted_files,
              preview_plan_token)
             VALUES ('preview', 'restore_preview', 'instance', 'completed', 2, 2,
                     1, 2, 3, 'token')",
        )
        .execute(&pool)
        .await
        .unwrap();
    let result: (i64, i64, i64, String) = sqlx::query_as(
        "SELECT preview_added_files, preview_modified_files,
                    preview_deleted_files, preview_plan_token
             FROM backup_operations WHERE id = 'preview'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(result, (1, 2, 3, "token".to_string()));
}

#[tokio::test]
async fn repository_validation_rejects_foreign_key_violations() {
    let temp = tempfile::tempdir().unwrap();
    tokio::fs::create_dir_all(temp.path().join(OBJECTS_DIR))
        .await
        .unwrap();
    let options = SqliteConnectOptions::new()
        .filename(temp.path().join(DATABASE_FILE))
        .create_if_missing(true)
        .foreign_keys(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    BACKUP_MIGRATOR.run(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO backup_snapshots
             (id, instance_id, instance_name, created_at, file_count,
              symlink_count, logical_size, added_size)
             VALUES ('snapshot', 'missing', 'Missing', 0, 0, 0, 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    let error = validate_repository_at(temp.path()).await.unwrap_err();
    assert!(error.to_string().contains("foreign key check"));
}

#[tokio::test]
async fn default_repository_follows_launcher_directory_move_idempotently() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("old/Backups/instances");
    let destination = temp.path().join("new/Backups/instances");
    let pool = open_repository_at(&source, true).await.unwrap().unwrap();
    pool.close().await;

    let pending =
        move_default_repository_for_launcher_directory(&source, &destination)
            .await
            .unwrap();
    assert!(pending.is_none());
    assert!(!source.exists());
    validate_repository_at(&destination).await.unwrap();

    let repeated =
        move_default_repository_for_launcher_directory(&source, &destination)
            .await
            .unwrap();
    assert!(repeated.is_none());
    validate_repository_at(&destination).await.unwrap();
}

#[tokio::test]
async fn full_instance_scan_includes_future_content_and_honors_exclusions() {
    let temp = tempfile::tempdir().unwrap();
    let instance = temp.path().join("instance");
    let repository = temp.path().join("repository");
    let staging = repository.join(STAGING_DIR).join("first");
    tokio::fs::create_dir_all(instance.join("config"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(instance.join("caches"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(repository.join(OBJECTS_DIR))
        .await
        .unwrap();
    tokio::fs::create_dir_all(&staging).await.unwrap();
    tokio::fs::write(instance.join("config/first.toml"), b"same")
        .await
        .unwrap();
    tokio::fs::write(instance.join("config/bigdb.sqlite"), b"ignored")
        .await
        .unwrap();
    tokio::fs::write(instance.join("caches/data.bin"), b"ignored")
        .await
        .unwrap();
    tokio::fs::write(instance.join("options.txt"), b"root")
        .await
        .unwrap();
    let exclusions = [
        BackupExclusion {
            path: "caches".to_string(),
            kind: BackupExclusionKind::Directory,
        },
        BackupExclusion {
            path: "config/bigdb.sqlite".to_string(),
            kind: BackupExclusionKind::File,
        },
    ];

    let mut first_objects = HashSet::new();
    let (first_entries, first_added) = scan_snapshot(
        &instance,
        &repository,
        &staging,
        &exclusions,
        Uuid::new_v4(),
        "instance",
        0,
        &CancellationToken::new(),
        &mut first_objects,
    )
    .await
    .unwrap();
    assert_eq!(
        first_entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .count(),
        2
    );
    assert!(
        first_entries
            .iter()
            .any(|entry| entry.path == "options.txt")
    );
    assert!(
        !first_entries
            .iter()
            .any(|entry| entry.path.starts_with("caches"))
    );
    assert!(
        !first_entries
            .iter()
            .any(|entry| entry.path == "config/bigdb.sqlite")
    );
    assert_eq!(first_added, 8);

    tokio::fs::write(instance.join("config/second.toml"), b"new")
        .await
        .unwrap();
    tokio::fs::create_dir_all(instance.join("logs"))
        .await
        .unwrap();
    tokio::fs::write(instance.join("logs/latest.log"), b"future")
        .await
        .unwrap();
    let second_staging = repository.join(STAGING_DIR).join("second");
    tokio::fs::create_dir_all(&second_staging).await.unwrap();
    let mut second_objects = HashSet::new();
    let (second_entries, second_added) = scan_snapshot(
        &instance,
        &repository,
        &second_staging,
        &exclusions,
        Uuid::new_v4(),
        "instance",
        0,
        &CancellationToken::new(),
        &mut second_objects,
    )
    .await
    .unwrap();
    assert_eq!(
        second_entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .count(),
        4
    );
    assert!(
        second_entries
            .iter()
            .any(|entry| entry.path == "logs/latest.log")
    );
    assert_eq!(second_added, 9, "only new content should be stored");
    assert_eq!(second_objects.len(), 2);
}

#[tokio::test]
async fn deleting_last_snapshot_reference_collects_the_object() {
    let temp = tempfile::tempdir().unwrap();
    let repository = temp.path();
    tokio::fs::create_dir_all(repository.join(OBJECTS_DIR))
        .await
        .unwrap();
    let options = SqliteConnectOptions::new()
        .filename(repository.join(DATABASE_FILE))
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    BACKUP_MIGRATOR.run(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO instance_backup_configs
             (instance_id, instance_name, created_at, modified_at)
             VALUES ('instance', 'Instance', 1, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let hash = "a".repeat(64);
    let object = object_path(repository, &hash);
    tokio::fs::create_dir_all(object.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&object, b"same").await.unwrap();
    sqlx::query(
        "INSERT INTO backup_objects (hash, size, ref_count)
             VALUES (?, 4, 2)",
    )
    .bind(&hash)
    .execute(&pool)
    .await
    .unwrap();
    for snapshot in ["first", "second"] {
        sqlx::query(
            "INSERT INTO backup_snapshots
                 (id, instance_id, instance_name, created_at, file_count,
                  symlink_count, logical_size, added_size)
                 VALUES (?, 'instance', 'Instance', 1, 1, 0, 4, 0)",
        )
        .bind(snapshot)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO snapshot_entries
                 (snapshot_id, path, kind, object_hash, size)
                 VALUES (?, 'config/file', 'file', ?, 4)",
        )
        .bind(snapshot)
        .bind(&hash)
        .execute(&pool)
        .await
        .unwrap();
    }

    delete_snapshots_in_pool(repository, &pool, Some("first"), None)
        .await
        .unwrap();
    assert!(object.exists());
    let ref_count: i64 = sqlx::query_scalar(
        "SELECT ref_count FROM backup_objects WHERE hash = ?",
    )
    .bind(&hash)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(ref_count, 1);

    delete_snapshots_in_pool(repository, &pool, Some("second"), None)
        .await
        .unwrap();
    assert!(!object.exists());
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM backup_objects WHERE hash = ?)",
    )
    .bind(&hash)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!exists);
}

#[tokio::test]
async fn interrupted_restore_rolls_back_every_selected_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("instance");
    let operation_root = temp.path().join("operation");
    tokio::fs::create_dir_all(operation_root.join("rollback/config"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(root.join("config"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(root.join("saves")).await.unwrap();
    tokio::fs::write(
        operation_root.join("rollback/config/value.txt"),
        b"before",
    )
    .await
    .unwrap();
    tokio::fs::write(root.join("config/value.txt"), b"restored")
        .await
        .unwrap();
    tokio::fs::write(root.join("saves/new.txt"), b"restored")
        .await
        .unwrap();

    recover_restore_operation(
        &root,
        &operation_root,
        &[
            RestoreJournalRoot {
                path: "config".to_string(),
                had_current: true,
            },
            RestoreJournalRoot {
                path: "saves".to_string(),
                had_current: false,
            },
        ],
    )
    .await
    .unwrap();

    assert_eq!(
        tokio::fs::read(root.join("config/value.txt"))
            .await
            .unwrap(),
        b"before"
    );
    assert!(!root.join("saves").exists());
    assert!(!operation_root.exists());
}

#[tokio::test]
async fn interrupted_whole_root_restore_preserves_excluded_content() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("instance");
    let operation_root = temp.path().join("operation");
    let rollback_root = operation_root.join("rollback/instance");
    tokio::fs::create_dir_all(rollback_root.join("config"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(root.join("caches"))
        .await
        .unwrap();
    tokio::fs::write(rollback_root.join("config/value.txt"), b"before")
        .await
        .unwrap();
    tokio::fs::write(root.join("config.txt"), b"restored")
        .await
        .unwrap();
    tokio::fs::write(root.join("caches/data.bin"), b"preserved")
        .await
        .unwrap();

    recover_whole_root_restore(
        &root,
        &operation_root,
        &[BackupExclusion {
            path: "caches".to_string(),
            kind: BackupExclusionKind::Directory,
        }],
    )
    .await
    .unwrap();

    assert_eq!(
        tokio::fs::read(root.join("config/value.txt"))
            .await
            .unwrap(),
        b"before"
    );
    assert_eq!(
        tokio::fs::read(root.join("caches/data.bin")).await.unwrap(),
        b"preserved"
    );
    assert!(!root.join("config.txt").exists());
    assert!(!operation_root.exists());
}

#[tokio::test]
async fn whole_root_restore_replaces_managed_content_and_keeps_exclusions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("instance");
    let operation_root = temp.path().join("operation");
    tokio::fs::create_dir_all(root.join("caches"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(operation_root.join("staged/config"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(operation_root.join("rollback"))
        .await
        .unwrap();
    tokio::fs::write(root.join("old.txt"), b"old")
        .await
        .unwrap();
    tokio::fs::write(root.join("caches/data.bin"), b"preserved")
        .await
        .unwrap();
    tokio::fs::write(
        operation_root.join("staged/config/value.txt"),
        b"snapshot",
    )
    .await
    .unwrap();

    replace_whole_root(
        &root,
        &operation_root,
        &[BackupExclusion {
            path: "caches".to_string(),
            kind: BackupExclusionKind::Directory,
        }],
    )
    .await
    .unwrap();

    assert_eq!(
        tokio::fs::read(root.join("config/value.txt"))
            .await
            .unwrap(),
        b"snapshot"
    );
    assert_eq!(
        tokio::fs::read(root.join("caches/data.bin")).await.unwrap(),
        b"preserved"
    );
    assert!(!root.join("old.txt").exists());
    assert_eq!(
        tokio::fs::read(operation_root.join("rollback/instance/old.txt"))
            .await
            .unwrap(),
        b"old"
    );
}
