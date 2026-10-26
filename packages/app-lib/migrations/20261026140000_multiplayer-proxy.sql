ALTER TABLE settings
ADD COLUMN multiplayer_proxy TEXT NOT NULL DEFAULT 'direct'
CHECK (multiplayer_proxy IN ('direct', 'configured'));
