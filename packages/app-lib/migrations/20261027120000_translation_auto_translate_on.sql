-- Automatic translation is now enabled by default so that opening a project page
-- translates its description without an extra click. SQLite cannot change a
-- column default in place, and the stored row already carries the old default,
-- so flip that row instead. Users who switch it off again keep their choice.
UPDATE translation_settings SET auto_translate = 1
WHERE id = 0 AND auto_translate = 0;
