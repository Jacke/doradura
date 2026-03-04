-- Track bot UI messages during player mode for cleanup on exit
CREATE TABLE player_messages (
    user_id    INTEGER NOT NULL,
    message_id INTEGER NOT NULL,
    PRIMARY KEY (user_id, message_id)
);
CREATE INDEX idx_player_messages_user ON player_messages(user_id);

-- Track pinned sticker for unpin on exit
ALTER TABLE player_sessions ADD COLUMN sticker_message_id INTEGER;
