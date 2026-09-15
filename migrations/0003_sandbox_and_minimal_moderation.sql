DROP TABLE mod_cases;

CREATE TABLE temp_bans (
    guild_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (guild_id, user_id)
);
CREATE INDEX temp_bans_expires_at ON temp_bans (expires_at);

CREATE TABLE channel_locks (
    channel_id INTEGER PRIMARY KEY,
    guild_id INTEGER NOT NULL,
    overwrites TEXT NOT NULL
);
