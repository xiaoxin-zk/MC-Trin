ALTER TABLE settings ADD COLUMN close_behavior TEXT NOT NULL DEFAULT 'ask' CHECK (
	close_behavior IN ('ask', 'close', 'lightweight')
);
