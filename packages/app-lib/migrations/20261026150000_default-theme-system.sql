-- The launcher now follows the OS colour scheme by default instead of forcing
-- the historical dark palette on every fresh or untouched profile.
UPDATE settings SET theme = 'system'
WHERE id = 0 AND theme = 'dark';
