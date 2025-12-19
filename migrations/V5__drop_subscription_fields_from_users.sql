-- Remove legacy subscription columns from users table (data already moved to subscriptions)
CREATE TABLE users_new (
    id INTEGER PRIMARY KEY,
    telegram_id INTEGER UNIQUE NOT NULL,
    username TEXT,
    plan TEXT DEFAULT 'free',
    download_format TEXT DEFAULT 'mp3',
    download_subtitles INTEGER DEFAULT 0,
    video_quality TEXT DEFAULT 'best',
    audio_bitrate TEXT DEFAULT '320k',
    send_as_document INTEGER DEFAULT 0,
    send_audio_as_document INTEGER DEFAULT 0,
    language TEXT DEFAULT 'ru'
);

INSERT INTO users_new (
    id,
    telegram_id,
    username,
    plan,
    download_format,
    download_subtitles,
    video_quality,
    audio_bitrate,
    send_as_document,
    send_audio_as_document,
    language
) SELECT
    id,
    telegram_id,
    username,
    plan,
    download_format,
    download_subtitles,
    video_quality,
    audio_bitrate,
    send_as_document,
    send_audio_as_document,
    COALESCE(language, 'ru')
FROM users;

DROP TABLE users;
ALTER TABLE users_new RENAME TO users;
