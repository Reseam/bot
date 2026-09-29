CREATE TABLE reminders (
    id INTEGER PRIMARY KEY,
    guild_id INTEGER NOT NULL,
    channel_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    message TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    due_at INTEGER NOT NULL
);
CREATE INDEX reminders_due_at ON reminders (due_at);
CREATE INDEX reminders_user ON reminders (guild_id, user_id);
