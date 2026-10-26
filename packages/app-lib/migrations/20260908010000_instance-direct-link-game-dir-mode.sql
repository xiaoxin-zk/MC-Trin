-- The selected external-root layout is recorded per linked instance so it
-- remains stable after Settings closes and across application restarts.
-- NULL keeps the legacy launcher-specific automatic behavior.
ALTER TABLE instances ADD COLUMN linked_game_dir_mode TEXT NULL;
