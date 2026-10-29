use super::*;

pub(crate) fn backup_relative_path_for_update(
    old_path: &str,
    new_path: &str,
) -> Option<String> {
    let old_name = Path::new(old_path).file_name()?.to_str()?;
    let new_name = Path::new(new_path).file_name()?.to_str()?;
    let old_base = old_name.trim_end_matches(".disabled");
    let new_base = new_name.trim_end_matches(".disabled");
    if old_base.is_empty() || new_base.is_empty() || old_base == new_base {
        return None;
    }
    let backup_name = format!("{new_base}_{old_base}.old");
    let directory = Path::new(old_path).parent()?.to_str()?;
    Some(format!("{directory}/{backup_name}"))
}
pub(crate) async fn archive_project_file(
    instance_id: &str,
    old_path: &str,
    new_path: &str,
    state: &State,
) -> crate::Result<Option<String>> {
    let Some(backup_relative_path) =
        backup_relative_path_for_update(old_path, new_path)
    else {
        return Ok(None);
    };
    let _instance_lock = state.lock_instance_content(instance_id).await;

    let scope = resolve_content_scope(instance_id, None, state).await?;
    let base = instance_full_path(state, &scope.instance);
    let full_old = join_content_path(&base, old_path);
    if !full_old.exists() {
        return Ok(None);
    }
    let full_backup = join_content_path(&base, &backup_relative_path);
    if full_backup.exists() {
        return Err(crate::ErrorKind::InputError(
            "Existing backup must be reviewed before it can be replaced".into(),
        )
        .into());
    }
    io::rename_or_move(&full_old, &full_backup).await?;
    touch_file_modified_time(&full_backup);

    if let Some(file) = content_rows::get_instance_file_by_relative_path(
        &scope.instance.id,
        old_path,
        &state.pool,
    )
    .await?
    {
        content_rows::remove_content_entries_for_file(
            &scope.content_set_id,
            &file.id,
            &state.pool,
        )
        .await?;
        content_rows::remove_instance_file_by_relative_path(
            &scope.instance.id,
            old_path,
            &state.pool,
        )
        .await?;
    }

    Ok(Some(backup_relative_path))
}

pub(crate) async fn rollback_project(
    instance_id: &str,
    project_path: &str,
    state: &std::sync::Arc<State>,
) -> crate::Result<String> {
    let instance_id = instance_id.to_owned();
    let project_path = project_path.to_owned();
    let state = state.clone();
    tokio::spawn(async move {
        rollback_project_inner(&instance_id, &project_path, &state).await
    })
    .await?
}

async fn rollback_project_inner(
    instance_id: &str,
    project_path: &str,
    state: &State,
) -> crate::Result<String> {
    let _writer = state.acquire_install_db_permit().await?;
    let _instance_lock =
        state.lock_instance_content_exclusive(instance_id).await;

    let scope = resolve_content_scope(instance_id, None, state).await?;
    let base = instance_content_root(&state.directories, &scope.instance)?;
    recover_legacy_rollback_for_instance(instance_id, &base, state).await?;
    content_rows::get_instance_file_by_relative_path(
        &scope.instance.id,
        project_path,
        &state.pool,
    )
    .await?
    .ok_or_else(|| {
        crate::ErrorKind::InputError(format!(
            "Project file '{project_path}' not found"
        ))
    })?;

    let active_name = Path::new(project_path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let active_base = active_name.trim_end_matches(".disabled");
    let active_dir = Path::new(project_path)
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default();
    let prefix = format!("{active_base}_");
    let folder = base.join(&active_dir);
    let backups = scan_folder_backups(&folder, &prefix)?;
    let Some((backup_name, _)) = backups.into_iter().max_by(|left, right| {
        left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0))
    }) else {
        return Err(crate::ErrorKind::InputError(format!(
            "No backup found for '{project_path}'"
        ))
        .into());
    };
    let old_base = backup_name
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix(".old"))
        .ok_or_else(|| {
            crate::ErrorKind::InputError(format!(
                "Invalid backup name '{backup_name}'"
            ))
        })?;
    if old_base.is_empty() || old_base == active_base {
        return Err(crate::ErrorKind::InputError(format!(
            "Invalid backup name '{backup_name}'"
        ))
        .into());
    }

    let disabled = active_name.ends_with(".disabled");
    let full_active =
        crate::resources::execution::checked_path(&base, project_path)?;
    let full_backup = crate::resources::execution::checked_path(
        &base,
        &format!("{active_dir}/{backup_name}"),
    )?;
    let restored_name = old_base.to_string();
    let restored_relative_path = format!("{active_dir}/{restored_name}");
    let full_restored = crate::resources::execution::checked_path(
        &base,
        &restored_relative_path,
    )?;
    let archive_name = format!("{old_base}_{active_base}.old");
    let full_archive = crate::resources::execution::checked_path(
        &base,
        &format!("{active_dir}/{archive_name}"),
    )?;
    let final_path = if disabled {
        format!("{restored_relative_path}.disabled")
    } else {
        restored_relative_path.clone()
    };
    let full_final =
        crate::resources::execution::checked_path(&base, &final_path)?;
    if full_restored.exists() || full_final.exists() {
        return Err(crate::ErrorKind::InputError(
            "Rollback destination already exists".into(),
        )
        .into());
    }

    let (restored_size, restored_sha1) =
        fetch::sha1_file_async(&full_backup).await?;

    if full_archive.exists() {
        return Err(crate::ErrorKind::InputError("Existing rollback archive must be reviewed before it can be replaced".into()).into());
    }
    let (active_size, active_sha1) =
        fetch::sha1_file_async(&full_active).await?;
    let journal = LegacyRollbackJournal {
        active: project_path.into(),
        backup: format!("{active_dir}/{backup_name}"),
        restored: restored_relative_path.clone(),
        final_path: final_path.clone(),
        archive: format!("{active_dir}/{archive_name}"),
        active_size,
        active_sha1,
        backup_size: restored_size,
        backup_sha1: restored_sha1.clone(),
    };
    sqlx::query("INSERT INTO resource_legacy_rollback_journals(instance_id, payload) VALUES (?, ?)").bind(instance_id).bind(serde_json::to_string(&journal)?).execute(&state.pool).await?;
    let _critical = crate::install::critical_section::enter();
    let result: crate::Result<String> = async {
    io::rename_or_move(&full_active, &full_archive).await?;
    touch_file_modified_time(&full_archive);
    io::rename_or_move(&full_backup, &full_restored).await?;

    let final_relative_path = if disabled {
        let disabled_path = format!("{restored_relative_path}.disabled");
        io::rename_or_move(
            &full_restored,
            &join_content_path(&base, &disabled_path),
        )
        .await?;
        disabled_path
    } else {
        restored_relative_path
    };
    let final_name = Path::new(&final_relative_path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let local_mod_data = if project_type_for_file_path(&final_relative_path)
        == Some(ProjectType::Mod)
    {
        tokio::fs::read(join_content_path(&base, &final_relative_path))
            .await
            .ok()
            .and_then(|data| {
                crate::mod_metadata::extract_mod_metadata(&bytes::Bytes::from(
                    data,
                ))
                .and_then(|meta| serde_json::to_string(&meta).ok())
            })
    } else {
        None
    };

    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE instance_files SET relative_path = ?, file_name = ?, enabled = ?, local_mod_data = NULL, icon_path = NULL WHERE instance_id = ? AND relative_path = ?").bind(&final_relative_path).bind(&final_name).bind(!disabled).bind(&scope.instance.id).bind(project_path).execute(&mut *tx).await?;
    content_rows::upsert_instance_file_from_parts_in_transaction(
        content_rows::UpsertInstanceFile {
            instance_id: &scope.instance.id,
            relative_path: &final_relative_path,
            file_name: &final_name,
            enabled: !disabled,
            sha1: &restored_sha1,
            size: restored_size,
            missing: false,
            local_mod_data: local_mod_data.as_deref(),
            icon_path: None,
        },
        &mut tx,
    )
    .await?;

    let entry_id: Option<String> = sqlx::query_scalar("SELECT entry.id FROM instance_content_entries entry JOIN instance_files file ON file.id = entry.file_id WHERE entry.content_set_id = ? AND file.instance_id = ? AND file.relative_path = ?")
        .bind(&scope.content_set_id).bind(&scope.instance.id).bind(&final_relative_path).fetch_optional(&mut *tx).await?;
    if let Some(entry_id) = entry_id {
        sqlx::query("DELETE FROM instance_content_provider_refs WHERE content_entry_id = ?").bind(&entry_id).execute(&mut *tx).await?;
        sqlx::query(
            "DELETE FROM resource_installed_artifacts WHERE entry_id = ?",
        )
        .bind(&entry_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM resource_entry_graphs WHERE entry_id = ?")
            .bind(&entry_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM instance_content_dependencies WHERE parent_entry_id = ?").bind(&entry_id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM instance_content_update_checks WHERE content_entry_id = ?").bind(&entry_id).execute(&mut *tx).await?;
    }
    sqlx::query("DELETE FROM store_instance_files WHERE instance_id = ? AND relative_path IN (?, ?)").bind(instance_id).bind(project_path).bind(&final_relative_path).execute(&mut *tx).await?;
    let hash_cache = CacheValue::FileHash(CachedFileHash {
        path: format!("{}/{}", scope.instance.path, final_relative_path),
        size: restored_size,
        hash: restored_sha1,
        project_type: project_type_for_file_path(&final_relative_path),
        project_id: None,
        version_id: None,
    })
    .get_entry();
    CachedEntry::upsert_many(&[hash_cache], &mut *tx).await?;
    content_rows::bump_content_set_revision_in_transaction(
        &scope.content_set_id,
        &mut tx,
    )
    .await?;
    sqlx::query("DELETE FROM resource_legacy_rollback_journals WHERE instance_id = ?").bind(instance_id).execute(&mut *tx).await?;
    tx.commit().await?;

    Ok(final_relative_path)
    }.await;
    if result.is_err() {
        recover_legacy_rollback_for_instance(instance_id, &base, state).await?;
    }
    result
}

#[derive(serde::Serialize, serde::Deserialize)]
struct LegacyRollbackJournal {
    active: String,
    backup: String,
    restored: String,
    final_path: String,
    archive: String,
    active_size: u64,
    active_sha1: String,
    backup_size: u64,
    backup_sha1: String,
}

async fn recover_legacy_rollback_for_instance(
    instance_id: &str,
    root: &Path,
    state: &State,
) -> crate::Result<()> {
    let payload: Option<String> = sqlx::query_scalar("SELECT payload FROM resource_legacy_rollback_journals WHERE instance_id = ?").bind(instance_id).fetch_optional(&state.pool).await?;
    let Some(payload) = payload else {
        return Ok(());
    };
    let journal: LegacyRollbackJournal = serde_json::from_str(&payload)?;
    let path = |relative: &str| {
        crate::resources::execution::checked_path(root, relative)
    };
    let active = path(&journal.active)?;
    let backup = path(&journal.backup)?;
    let restored = path(&journal.restored)?;
    let final_path = path(&journal.final_path)?;
    let archive = path(&journal.archive)?;
    let matches = |size, hash: &str| crate::resources::ArtifactIntegrity {
        size: Some(size),
        hashes: vec![crate::resources::ContentHash {
            algorithm: crate::resources::HashAlgorithm::Sha1,
            value: hash.into(),
        }],
    };
    let _critical = crate::install::critical_section::enter();
    if !backup.exists() {
        let candidate = if final_path.exists() {
            &final_path
        } else {
            &restored
        };
        if !crate::resources::execution::matches_file(
            candidate,
            &matches(journal.backup_size, &journal.backup_sha1),
        )
        .await?
        {
            return Err(crate::ErrorKind::InputError(
                "Rollback backup could not be recovered".into(),
            )
            .into());
        }
        io::rename_or_move(candidate, &backup).await?;
    } else if !crate::resources::execution::matches_file(
        &backup,
        &matches(journal.backup_size, &journal.backup_sha1),
    )
    .await?
    {
        return Err(crate::ErrorKind::InputError(
            "Rollback backup changed during recovery".into(),
        )
        .into());
    }
    if !active.exists() {
        if !crate::resources::execution::matches_file(
            &archive,
            &matches(journal.active_size, &journal.active_sha1),
        )
        .await?
        {
            return Err(crate::ErrorKind::InputError(
                "Rollback active file could not be recovered".into(),
            )
            .into());
        }
        io::rename_or_move(&archive, &active).await?;
    } else if !crate::resources::execution::matches_file(
        &active,
        &matches(journal.active_size, &journal.active_sha1),
    )
    .await?
    {
        return Err(crate::ErrorKind::InputError(
            "Rollback active file changed during recovery".into(),
        )
        .into());
    }
    sqlx::query("DELETE FROM resource_legacy_rollback_journals WHERE instance_id = ? AND payload = ?").bind(instance_id).bind(payload).execute(&state.pool).await?;
    Ok(())
}

pub(crate) async fn recover_legacy_rollbacks(
    state: &State,
) -> crate::Result<()> {
    let instances: Vec<String> = sqlx::query_scalar(
        "SELECT instance_id FROM resource_legacy_rollback_journals",
    )
    .fetch_all(&state.pool)
    .await?;
    for instance_id in instances {
        let _writer = state.acquire_install_db_permit().await?;
        let _lock = state.lock_instance_content_exclusive(&instance_id).await;
        let scope = resolve_content_scope(&instance_id, None, state).await?;
        let root = instance_content_root(&state.directories, &scope.instance)?;
        recover_legacy_rollback_for_instance(&instance_id, &root, state)
            .await?;
    }
    Ok(())
}

fn scan_folder_backups(
    folder: &Path,
    prefix: &str,
) -> crate::Result<Vec<(String, i64)>> {
    let mut backups = Vec::new();
    for entry in std::fs::read_dir(folder)
        .map_err(|err| io::IOError::with_path(err, folder))?
    {
        let path = entry.map_err(io::IOError::from)?.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|value| value.to_str())
        else {
            continue;
        };
        if !file_name.starts_with(prefix) || !file_name.ends_with(".old") {
            continue;
        }
        let modified = path
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default();
        backups.push((file_name.to_string(), modified));
    }
    Ok(backups)
}

fn touch_file_modified_time(path: &Path) {
    let Ok(file) = std::fs::File::options().write(true).open(path) else {
        return;
    };
    let _ = file.set_modified(std::time::SystemTime::now());
}
