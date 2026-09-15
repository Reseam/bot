CREATE TABLE conversations (
    id INTEGER PRIMARY KEY,
    guild_id INTEGER NOT NULL,
    channel_id INTEGER NOT NULL,
    started_by INTEGER NOT NULL,
    transcript TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX conversations_updated_at ON conversations (updated_at);

CREATE TABLE conversation_messages (
    message_id INTEGER PRIMARY KEY,
    conversation_id INTEGER NOT NULL REFERENCES conversations (id) ON DELETE CASCADE
);
CREATE INDEX conversation_messages_conversation ON conversation_messages (conversation_id);
