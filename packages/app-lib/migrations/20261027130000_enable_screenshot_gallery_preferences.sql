-- Preserve the existing global screenshot gallery contents while activating
-- the previously stored global and per-instance screenshot preferences.
UPDATE sync_feature_settings
SET globally_enabled = 1, new_instance_default = 1
WHERE feature = 'screenshots';

UPDATE instance_sync_preferences
SET enabled = 1
WHERE feature = 'screenshots';
