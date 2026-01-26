-- User media uploads table for premium/vip users
-- Stores uploaded photos, videos, documents for later conversion

CREATE TABLE IF NOT EXISTS uploads (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    -- Original filename from Telegram
    original_filename TEXT,
    -- Display title (can be renamed by user)
    title TEXT NOT NULL,
    -- Media type: 'photo', 'video', 'document', 'audio'
    media_type TEXT NOT NULL,
    -- File format/extension: 'mp4', 'jpg', 'png', 'docx', 'pdf', etc.
    file_format TEXT,
    -- Telegram file_id for retrieval
    file_id TEXT NOT NULL,
    -- Telegram file_unique_id for deduplication
    file_unique_id TEXT,
    -- File size in bytes
    file_size INTEGER,
    -- Duration in seconds (for video/audio)
    duration INTEGER,
    -- Width in pixels (for photo/video)
    width INTEGER,
    -- Height in pixels (for photo/video)
    height INTEGER,
    -- MIME type
    mime_type TEXT,
    -- Message ID where file was sent (for MTProto fallback)
    message_id INTEGER,
    -- Chat ID where message was sent
    chat_id INTEGER,
    -- Upload timestamp
    uploaded_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    -- Thumbnail file_id (for video)
    thumbnail_file_id TEXT,
    FOREIGN KEY (user_id) REFERENCES users(telegram_id)
);

CREATE INDEX IF NOT EXISTS idx_uploads_user_id ON uploads(user_id);
CREATE INDEX IF NOT EXISTS idx_uploads_uploaded_at ON uploads(uploaded_at DESC);
CREATE INDEX IF NOT EXISTS idx_uploads_media_type ON uploads(media_type);
CREATE INDEX IF NOT EXISTS idx_uploads_file_unique_id ON uploads(file_unique_id);
