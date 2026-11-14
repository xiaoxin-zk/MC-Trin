CREATE TABLE assistant_ai_settings (
	id INTEGER NOT NULL CHECK (id = 0),
	provider_id TEXT NOT NULL DEFAULT '',
	model_id TEXT NOT NULL DEFAULT '',
	PRIMARY KEY (id)
);

INSERT INTO assistant_ai_settings (id) VALUES (0);
