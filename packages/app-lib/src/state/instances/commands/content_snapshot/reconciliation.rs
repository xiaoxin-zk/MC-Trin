use std::collections::{HashMap, HashSet};

use sqlx::Row;

use super::CONTENT_LOCK_TIMEOUT;
use crate::State;
use crate::state::instances::adapters::{filesystem, sqlite::content_rows};
use crate::state::instances::{
    ContentOwnershipKind, ManualDownloadOperationKind, ManualDownloadState,
    PackMemberMaterializationState, PackMemberOverrideKind,
    PendingManualDownload,
};
use crate::state::{ContentProvider, ContentRequirement, ContentSourceKind};

#[derive(Clone)]
struct CurseForgeReconciliationCandidate {
    entry_id: String,
    relative_path: String,
    enabled: bool,
    missing: bool,
    provider_project_id: Option<String>,
    provider_release_id: Option<String>,
}

pub(crate) async fn reconcile_curseforge_members(
    instance_id: &str,
    content_set_id: &str,
    expected: &crate::api::curseforge::CurseForgePackExpectedContent,
    state: &State,
) -> crate::Result<()> {
    use chrono::Utc;

    let _instance_lock = state
        .lock_instance_content_with_timeout(instance_id, CONTENT_LOCK_TIMEOUT)
        .await?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let rows = sqlx::query(
        "SELECT entry.id AS entry_id, file.relative_path, entry.enabled,
			file.missing, ref.provider_project_id, ref.provider_release_id
		 FROM instance_content_entries entry
		 INNER JOIN instance_files file ON file.id = entry.file_id
		 LEFT JOIN instance_content_provider_refs ref
			ON ref.content_entry_id = entry.id AND ref.provider = 'curseforge'
		 WHERE entry.content_set_id = ?
		 ORDER BY entry.modified_at DESC",
    )
    .bind(content_set_id)
    .fetch_all(&mut *tx)
    .await?;
    let candidates = rows
        .into_iter()
        .map(|row| {
            Ok(CurseForgeReconciliationCandidate {
                entry_id: row.try_get("entry_id")?,
                relative_path: row.try_get("relative_path")?,
                enabled: row.try_get::<i64, _>("enabled")? != 0,
                missing: row.try_get::<i64, _>("missing")? != 0,
                provider_project_id: row.try_get("provider_project_id")?,
                provider_release_id: row.try_get("provider_release_id")?,
            })
        })
        .collect::<crate::Result<Vec<_>>>()?;
    let member_rows = sqlx::query(
        "SELECT id, member_key, override_kind, materialization_state,
            CASE WHEN provider IS NULL THEN expected_relative_path END AS override_path
		 FROM instance_pack_members WHERE content_set_id = ?",
    )
    .bind(content_set_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut existing_members = member_rows
        .into_iter()
        .map(|row| {
            let key = row.try_get::<String, _>("member_key")?;
            Ok((
                key,
                (
                    row.try_get::<String, _>("id")?,
                    PackMemberOverrideKind::from_str(
                        &row.try_get::<String, _>("override_kind")?,
                    )?,
                    PackMemberMaterializationState::from_str(
                        &row.try_get::<String, _>("materialization_state")?,
                    )?,
                    row.try_get::<Option<String>, _>("override_path")?,
                ),
            ))
        })
        .collect::<crate::Result<HashMap<_, _>>>()?;
    let pending_rows = sqlx::query(
        "SELECT provider_project_id, provider_release_id
		 FROM instance_pending_manual_downloads
		 WHERE instance_id = ? AND provider = 'curseforge'
			AND state IN ('waiting', 'matched', 'error')",
    )
    .bind(instance_id)
    .fetch_all(&mut *tx)
    .await?;
    let pending = pending_rows
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>("provider_project_id")?,
                row.try_get::<String, _>("provider_release_id")?,
            ))
        })
        .collect::<crate::Result<HashSet<_>>>()?;

    sqlx::query(
        "UPDATE instance_content_entries
		 SET ownership_kind = 'user_added', modified_at = ?
		 WHERE content_set_id = ? AND ownership_kind = 'pack_managed'",
    )
    .bind(Utc::now().timestamp())
    .bind(content_set_id)
    .execute(&mut *tx)
    .await?;

    let mut claimed_entries = HashSet::new();
    for expected_member in &expected.members {
        let project_id = expected_member.project_id.to_string();
        let file_id = expected_member.file_id.to_string();
        let expected_path =
            normalized_content_path(&expected_member.expected_relative_path);
        let candidate = candidates
            .iter()
            .find(|candidate| {
                !claimed_entries.contains(&candidate.entry_id)
                    && candidate.provider_project_id.as_deref()
                        == Some(project_id.as_str())
                    && candidate.provider_release_id.as_deref()
                        == Some(file_id.as_str())
            })
            .or_else(|| {
                candidates.iter().find(|candidate| {
                    !claimed_entries.contains(&candidate.entry_id)
                        && candidate.provider_project_id.as_deref()
                            == Some(project_id.as_str())
                })
            })
            .or_else(|| {
                candidates.iter().find(|candidate| {
                    !claimed_entries.contains(&candidate.entry_id)
                        && normalized_content_path(&candidate.relative_path)
                            == expected_path
                })
            });
        if let Some(candidate) = candidate {
            claimed_entries.insert(candidate.entry_id.clone());
            sqlx::query(
                "UPDATE instance_content_entries
				 SET ownership_kind = 'pack_managed', modified_at = ?
				 WHERE id = ?",
            )
            .bind(Utc::now().timestamp())
            .bind(&candidate.entry_id)
            .execute(&mut *tx)
            .await?;
        }

        let member_key = format!(
            "curseforge:{}:{}",
            expected_member.project_id,
            expected_member.project_type.get_name()
        );
        let existing = existing_members.remove(&member_key);
        let inherited_override = existing
            .as_ref()
            .map_or(PackMemberOverrideKind::None, |(_, kind, _, _)| *kind);
        let override_kind =
            if inherited_override != PackMemberOverrideKind::None {
                inherited_override
            } else if candidate.is_some_and(|candidate| {
                candidate.provider_project_id.as_deref()
                    == Some(project_id.as_str())
                    && candidate.provider_release_id.as_deref()
                        != Some(file_id.as_str())
            }) {
                PackMemberOverrideKind::Version
            } else if candidate.is_some_and(|candidate| !candidate.enabled) {
                PackMemberOverrideKind::Disabled
            } else {
                PackMemberOverrideKind::None
            };
        let materialization_state = if override_kind
            == PackMemberOverrideKind::Removed
            && candidate.is_none()
        {
            PackMemberMaterializationState::Removed
        } else if pending.contains(&(project_id.clone(), file_id.clone())) {
            PackMemberMaterializationState::PendingManual
        } else if candidate.is_some_and(|candidate| !candidate.missing) {
            PackMemberMaterializationState::Present
        } else {
            PackMemberMaterializationState::Missing
        };
        let now = Utc::now();
        let member_id = existing
            .map(|(id, _, _, _)| id)
            .unwrap_or_else(|| format!("pack-member:{}", uuid::Uuid::new_v4()));
        content_rows::upsert_pack_member_in_transaction(
            &crate::state::instances::PackMember {
                id: member_id.clone(),
                content_set_id: content_set_id.to_string(),
                content_entry_id: candidate
                    .map(|candidate| candidate.entry_id.clone()),
                member_key,
                project_type: expected_member.project_type,
                expected_relative_path: expected_member
                    .expected_relative_path
                    .clone(),
                provider: Some(ContentProvider::CurseForge),
                provider_project_id: Some(project_id),
                provider_release_id: Some(file_id),
                required: expected_member.required,
                expected_sha1: expected_member.expected_sha1.clone(),
                expected_size: expected_member.expected_size,
                expected_fingerprint: expected_member.expected_fingerprint,
                materialization_state,
                override_kind,
                reconciled: true,
                created_at: now,
                modified_at: now,
            },
            &mut tx,
        )
        .await?;
        sqlx::query(
            "UPDATE instance_pack_members
			 SET required = ?, reconciled = 1 WHERE id = ?",
        )
        .bind(i64::from(expected_member.required))
        .bind(member_id)
        .execute(&mut *tx)
        .await?;
    }

    for expected_override in &expected.overrides {
        let expected_path =
            normalized_content_path(&expected_override.expected_relative_path);
        existing_members.retain(|_, (_, _, _, path)| {
            path.as_deref().is_none_or(|path| {
                normalized_content_path(path) != expected_path
            })
        });
    }
    for (_, (member_id, _, _, _)) in existing_members {
        sqlx::query("DELETE FROM instance_pack_members WHERE id = ?")
            .bind(member_id)
            .execute(&mut *tx)
            .await?;
    }
    reconcile_curseforge_override_entries_in_transaction(
        instance_id,
        content_set_id,
        &expected.overrides,
        &mut tx,
    )
    .await?;
    content_rows::repair_pack_members_in_transaction(content_set_id, &mut tx)
        .await?;
    let override_paths: HashSet<_> = expected
        .overrides
        .iter()
        .map(|item| normalized_content_path(&item.expected_relative_path))
        .collect();
    let unreconciled: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, expected_relative_path FROM instance_pack_members
         WHERE content_set_id = ? AND provider IS NULL AND reconciled = 0",
    )
    .bind(content_set_id)
    .fetch_all(&mut *tx)
    .await?;
    for (member_id, path) in unreconciled {
        if override_paths.contains(&normalized_content_path(&path)) {
            sqlx::query(
                "UPDATE instance_pack_members SET reconciled = 1 WHERE id = ?",
            )
            .bind(member_id)
            .execute(&mut *tx)
            .await?;
        }
    }
    content_rows::bump_content_set_revision_in_transaction(
        content_set_id,
        &mut tx,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn reconcile_curseforge_override_entries_in_transaction(
    instance_id: &str,
    content_set_id: &str,
    expected: &[crate::api::curseforge::CurseForgePackExpectedOverride],
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> crate::Result<()> {
    let rows = sqlx::query(
        "SELECT file.id AS file_id, file.relative_path, file.enabled,
            (
                SELECT entry.id
                FROM instance_content_entries entry
                WHERE entry.content_set_id = ? AND entry.file_id = file.id
                ORDER BY entry.modified_at DESC
                LIMIT 1
            ) AS entry_id
         FROM instance_files file
         WHERE file.instance_id = ? AND file.missing = 0",
    )
    .bind(content_set_id)
    .bind(instance_id)
    .fetch_all(&mut **tx)
    .await?;
    let files = rows
        .into_iter()
        .map(|row| {
            let relative_path = row.try_get::<String, _>("relative_path")?;
            Ok((
                normalized_content_path(&relative_path),
                (
                    row.try_get::<String, _>("file_id")?,
                    row.try_get::<i64, _>("enabled")? != 0,
                    row.try_get::<Option<String>, _>("entry_id")?,
                ),
            ))
        })
        .collect::<crate::Result<HashMap<_, _>>>()?;
    let expected = expected
        .iter()
        .map(|item| {
            (normalized_content_path(&item.expected_relative_path), item)
        })
        .collect::<HashMap<_, _>>();

    for (relative_path, (file_id, enabled, entry_id)) in files {
        let override_content = expected.get(&relative_path).copied();
        if entry_id.is_some() && override_content.is_none() {
            continue;
        }
        let Some(project_type) =
            override_content.map(|item| item.project_type).or_else(|| {
                filesystem::project_type_from_relative_path(&relative_path)
            })
        else {
            continue;
        };
        let is_pack_override = override_content.is_some();
        content_rows::upsert_content_entry_from_parts_in_transaction(
            content_rows::UpsertContentEntry {
                instance_id,
                content_set_id,
                file_id: Some(&file_id),
                project_type,
                source_kind: if is_pack_override {
                    ContentSourceKind::CurseForge
                } else {
                    ContentSourceKind::Local
                },
                ownership_kind: if is_pack_override {
                    ContentOwnershipKind::PackManaged
                } else {
                    ContentOwnershipKind::UserAdded
                },
                auto_dependency: false,
                server_requirement: ContentRequirement::Required,
                client_requirement: ContentRequirement::Required,
                enabled,
            },
            tx,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn reconcile_curseforge_manual_downloads(
    instance_id: &str,
    content_set_id: &str,
    expected: &[crate::api::curseforge::CurseForgePackExpectedMember],
    state: &State,
) -> crate::Result<()> {
    let restricted = expected
        .iter()
        .filter(|member| member.required && member.manual_download.is_some())
        .collect::<Vec<_>>();
    if restricted.is_empty() {
        return Ok(());
    }

    let _instance_lock = state
        .lock_instance_content_with_timeout(instance_id, CONTENT_LOCK_TIMEOUT)
        .await?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let member_rows = sqlx::query(
        "SELECT id, content_entry_id, provider_project_id,
                provider_release_id, materialization_state, override_kind
         FROM instance_pack_members
         WHERE content_set_id = ? AND provider = 'curseforge'",
    )
    .bind(content_set_id)
    .fetch_all(&mut *tx)
    .await?;
    let members = member_rows
        .into_iter()
        .map(|row| {
            Ok((
                (
                    row.try_get::<Option<String>, _>("provider_project_id")?,
                    row.try_get::<Option<String>, _>("provider_release_id")?,
                ),
                (
                    row.try_get::<String, _>("id")?,
                    row.try_get::<Option<String>, _>("content_entry_id")?,
                    PackMemberMaterializationState::from_str(
                        &row.try_get::<String, _>("materialization_state")?,
                    )?,
                    PackMemberOverrideKind::from_str(
                        &row.try_get::<String, _>("override_kind")?,
                    )?,
                ),
            ))
        })
        .collect::<crate::Result<HashMap<_, _>>>()?;
    let pending_rows = sqlx::query(
        "SELECT provider_project_id, provider_release_id
         FROM instance_pending_manual_downloads
         WHERE instance_id = ? AND provider = 'curseforge'
            AND state IN ('waiting', 'matched', 'error')",
    )
    .bind(instance_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut pending = pending_rows
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>("provider_project_id")?,
                row.try_get::<String, _>("provider_release_id")?,
            ))
        })
        .collect::<crate::Result<HashSet<_>>>()?;
    let mut changed = false;

    for expected_member in restricted {
        let key = (
            Some(expected_member.project_id.to_string()),
            Some(expected_member.file_id.to_string()),
        );
        let Some((
            member_id,
            content_entry_id,
            materialization_state,
            override_kind,
        )) = members.get(&key)
        else {
            continue;
        };
        if matches!(
            *materialization_state,
            PackMemberMaterializationState::Present
                | PackMemberMaterializationState::Removed
        ) {
            continue;
        }

        let project_id = expected_member.project_id.to_string();
        let file_id = expected_member.file_id.to_string();
        if !pending.contains(&(project_id.clone(), file_id.clone())) {
            let Some(mut manual_download) =
                expected_member.manual_download.clone()
            else {
                continue;
            };
            manual_download.operation_kind =
                ManualDownloadOperationKind::PackInstall;
            let now = chrono::Utc::now();
            content_rows::upsert_pending_manual_download_in_transaction(
                &PendingManualDownload {
                    id: format!("manual-download:{}", uuid::Uuid::new_v4()),
                    instance_id: instance_id.to_string(),
                    pack_member_id: Some(member_id.clone()),
                    content_entry_id: content_entry_id.clone(),
                    operation_kind: ManualDownloadOperationKind::PackInstall,
                    operation_target_id: None,
                    project_type: expected_member.project_type,
                    provider: ContentProvider::CurseForge,
                    provider_project_id: project_id.clone(),
                    provider_release_id: file_id.clone(),
                    file_name: manual_download.file_name.clone(),
                    website_url: manual_download.website_url.clone(),
                    target_relative_path: expected_member
                        .expected_relative_path
                        .clone(),
                    expected_sha1: expected_member.expected_sha1.clone(),
                    expected_size: expected_member.expected_size,
                    expected_fingerprint: expected_member.expected_fingerprint,
                    state: ManualDownloadState::Waiting,
                    context: serde_json::to_value(&manual_download)?,
                    created_at: now,
                    modified_at: now,
                },
                &mut tx,
            )
            .await?;
            pending.insert((project_id, file_id));
            changed = true;
        }

        if *materialization_state
            != PackMemberMaterializationState::PendingManual
        {
            content_rows::set_pack_member_override_in_transaction(
                member_id,
                PackMemberMaterializationState::PendingManual,
                *override_kind,
                &mut tx,
            )
            .await?;
            changed = true;
        }
    }

    if changed {
        content_rows::bump_content_set_revision_in_transaction(
            content_set_id,
            &mut tx,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

fn normalized_content_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_end_matches(".disabled")
        .to_ascii_lowercase()
}
