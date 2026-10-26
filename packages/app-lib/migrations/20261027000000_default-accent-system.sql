-- Follow the OS accent colour by default, matching the system-following theme
-- default added in 20261026150000. Only the historical `pink` default value is
-- rewritten; a stored colour that is not the untouched default is left alone.
UPDATE settings SET accent_color = 'system'
WHERE id = 0 AND accent_color = 'pink';
