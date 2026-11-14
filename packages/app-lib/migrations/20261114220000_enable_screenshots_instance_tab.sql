-- The instance screenshots tab is now a real, user-facing setting instead of a
-- dead flag, and every other instance tab defaults to visible. Turn it on for
-- profiles that still carry the shipped-off default so the tab is reachable
-- without hunting for a setting. A database that already stored `1` is left
-- alone, and a user who turns it back off keeps that choice.
UPDATE settings SET show_screenshots_tab_in_instances = TRUE
WHERE id = 0 AND show_screenshots_tab_in_instances = FALSE;
