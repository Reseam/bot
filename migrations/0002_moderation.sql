CREATE TABLE mod_cases (
    id INTEGER PRIMARY KEY,
    guild_id INTEGER NOT NULL,
    action TEXT NOT NULL,
    target_id INTEGER,
    channel_id INTEGER,
    moderator_id INTEGER NOT NULL,
    reason TEXT NOT NULL,
    duration_secs INTEGER,
    expires_at INTEGER,
    resolved INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
CREATE INDEX mod_cases_target ON mod_cases (guild_id, target_id);
CREATE INDEX mod_cases_expiring ON mod_cases (expires_at) WHERE expires_at IS NOT NULL AND resolved = 0;

CREATE TABLE guild_settings (
    guild_id INTEGER PRIMARY KEY,
    mod_log_channel_id INTEGER
);
