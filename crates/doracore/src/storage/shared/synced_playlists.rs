use anyhow::{Context, Result};
use sqlx::Row;

use crate::storage::db::{self, SyncedPlaylist, SyncedTrack};

use super::SharedStorage;

impl SharedStorage {
    pub async fn create_synced_playlist(
        &self,
        user_id: i64,
        name: &str,
        description: Option<&str>,
        source_url: &str,
        source_platform: &str,
        track_count: i32,
        matched_count: i32,
        not_found_count: i32,
    ) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_synced_playlist connection")?;
                db::create_synced_playlist(
                    &conn,
                    user_id,
                    name,
                    description,
                    source_url,
                    source_platform,
                    track_count,
                    matched_count,
                    not_found_count,
                )
                .context("sqlite create_synced_playlist")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO synced_playlists (
                        user_id, name, description, source_url, source_platform,
                        track_count, matched_count, not_found_count, created_at, updated_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW(), NOW())
                     RETURNING id",
                )
                .bind(user_id)
                .bind(name)
                .bind(description)
                .bind(source_url)
                .bind(source_platform)
                .bind(track_count)
                .bind(matched_count)
                .bind(not_found_count)
                .fetch_one(pg_pool)
                .await
                .context("postgres create_synced_playlist")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn get_synced_playlist(&self, playlist_id: i64) -> Result<Option<SyncedPlaylist>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_synced_playlist connection")?;
                db::get_synced_playlist(&conn, playlist_id).context("sqlite get_synced_playlist")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, user_id, name, description, source_url, source_platform,
                            track_count, matched_count, not_found_count, sync_enabled,
                            CAST(last_synced_at AS TEXT) AS last_synced_at,
                            CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM synced_playlists
                     WHERE id = $1",
                )
                .bind(playlist_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_synced_playlist")?;
                row.map(map_pg_synced_playlist).transpose()
            }
        }
    }

    pub async fn get_user_synced_playlists(&self, user_id: i64) -> Result<Vec<SyncedPlaylist>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_synced_playlists connection")?;
                db::get_user_synced_playlists(&conn, user_id).context("sqlite get_user_synced_playlists")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, user_id, name, description, source_url, source_platform,
                            track_count, matched_count, not_found_count, sync_enabled,
                            CAST(last_synced_at AS TEXT) AS last_synced_at,
                            CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM synced_playlists
                     WHERE user_id = $1
                     ORDER BY created_at DESC",
                )
                .bind(user_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_synced_playlists")?;
                rows.into_iter().map(map_pg_synced_playlist).collect()
            }
        }
    }

    pub async fn get_synced_playlist_by_url(&self, user_id: i64, source_url: &str) -> Result<Option<SyncedPlaylist>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_synced_playlist_by_url connection")?;
                db::get_synced_playlist_by_url(&conn, user_id, source_url).context("sqlite get_synced_playlist_by_url")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, user_id, name, description, source_url, source_platform,
                            track_count, matched_count, not_found_count, sync_enabled,
                            CAST(last_synced_at AS TEXT) AS last_synced_at,
                            CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM synced_playlists
                     WHERE user_id = $1 AND source_url = $2",
                )
                .bind(user_id)
                .bind(source_url)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_synced_playlist_by_url")?;
                row.map(map_pg_synced_playlist).transpose()
            }
        }
    }

    pub async fn count_user_synced_playlists(&self, user_id: i64) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_user_synced_playlists connection")?;
                db::count_user_synced_playlists(&conn, user_id).context("sqlite count_user_synced_playlists")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT COUNT(*)::BIGINT AS count
                     FROM synced_playlists
                     WHERE user_id = $1",
                )
                .bind(user_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres count_user_synced_playlists")?;
                Ok(row.get("count"))
            }
        }
    }

    pub async fn update_synced_playlist_counts(
        &self,
        playlist_id: i64,
        track_count: i32,
        matched_count: i32,
        not_found_count: i32,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_synced_playlist_counts connection")?;
                db::update_synced_playlist_counts(&conn, playlist_id, track_count, matched_count, not_found_count)
                    .context("sqlite update_synced_playlist_counts")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE synced_playlists
                     SET track_count = $2,
                         matched_count = $3,
                         not_found_count = $4,
                         last_synced_at = NOW(),
                         updated_at = NOW()
                     WHERE id = $1",
                )
                .bind(playlist_id)
                .bind(track_count)
                .bind(matched_count)
                .bind(not_found_count)
                .execute(pg_pool)
                .await
                .context("postgres update_synced_playlist_counts")?;
                Ok(())
            }
        }
    }

    pub async fn delete_synced_playlist(&self, playlist_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_synced_playlist connection")?;
                db::delete_synced_playlist(&conn, playlist_id).context("sqlite delete_synced_playlist")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM synced_playlists WHERE id = $1")
                    .bind(playlist_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_synced_playlist")?;
                Ok(())
            }
        }
    }

    pub async fn increment_synced_playlist_matched(&self, playlist_id: i64, delta: i32) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn =
                    db::get_connection(db_pool).context("sqlite increment_synced_playlist_matched connection")?;
                db::increment_synced_playlist_matched(&conn, playlist_id, delta)
                    .context("sqlite increment_synced_playlist_matched")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE synced_playlists
                     SET matched_count = matched_count + $2,
                         not_found_count = not_found_count - $2,
                         updated_at = NOW()
                     WHERE id = $1",
                )
                .bind(playlist_id)
                .bind(delta)
                .execute(pg_pool)
                .await
                .context("postgres increment_synced_playlist_matched")?;
                Ok(())
            }
        }
    }

    pub async fn add_synced_track(
        &self,
        playlist_id: i64,
        position: i32,
        title: &str,
        artist: Option<&str>,
        duration_secs: Option<i32>,
        external_id: Option<&str>,
        source_url: Option<&str>,
        resolved_url: Option<&str>,
        import_status: &str,
    ) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite add_synced_track connection")?;
                db::add_synced_track(
                    &conn,
                    playlist_id,
                    position,
                    title,
                    artist,
                    duration_secs,
                    external_id,
                    source_url,
                    resolved_url,
                    import_status,
                )
                .context("sqlite add_synced_track")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO synced_tracks (
                        playlist_id, position, title, artist, duration_secs, external_id,
                        source_url, resolved_url, import_status, added_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
                     RETURNING id",
                )
                .bind(playlist_id)
                .bind(position)
                .bind(title)
                .bind(artist)
                .bind(duration_secs)
                .bind(external_id)
                .bind(source_url)
                .bind(resolved_url)
                .bind(import_status)
                .fetch_one(pg_pool)
                .await
                .context("postgres add_synced_track")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn get_synced_tracks(&self, playlist_id: i64) -> Result<Vec<SyncedTrack>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_synced_tracks connection")?;
                db::get_synced_tracks(&conn, playlist_id).context("sqlite get_synced_tracks")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, playlist_id, position, title, artist, duration_secs, external_id,
                            source_url, resolved_url, import_status, file_id,
                            CAST(added_at AS TEXT) AS added_at
                     FROM synced_tracks
                     WHERE playlist_id = $1
                     ORDER BY position ASC",
                )
                .bind(playlist_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_synced_tracks")?;
                rows.into_iter().map(map_pg_synced_track).collect()
            }
        }
    }

    pub async fn get_synced_tracks_page(&self, playlist_id: i64, offset: i64, limit: i64) -> Result<Vec<SyncedTrack>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_synced_tracks_page connection")?;
                db::get_synced_tracks_page(&conn, playlist_id, offset, limit).context("sqlite get_synced_tracks_page")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, playlist_id, position, title, artist, duration_secs, external_id,
                            source_url, resolved_url, import_status, file_id,
                            CAST(added_at AS TEXT) AS added_at
                     FROM synced_tracks
                     WHERE playlist_id = $1
                     ORDER BY position ASC
                     LIMIT $2 OFFSET $3",
                )
                .bind(playlist_id)
                .bind(limit)
                .bind(offset)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_synced_tracks_page")?;
                rows.into_iter().map(map_pg_synced_track).collect()
            }
        }
    }

    pub async fn get_synced_track(&self, track_id: i64) -> Result<Option<SyncedTrack>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_synced_track connection")?;
                db::get_synced_track(&conn, track_id).context("sqlite get_synced_track")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, playlist_id, position, title, artist, duration_secs, external_id,
                            source_url, resolved_url, import_status, file_id,
                            CAST(added_at AS TEXT) AS added_at
                     FROM synced_tracks
                     WHERE id = $1",
                )
                .bind(track_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_synced_track")?;
                row.map(map_pg_synced_track).transpose()
            }
        }
    }

    pub async fn update_synced_track_file_id(&self, track_id: i64, file_id: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_synced_track_file_id connection")?;
                db::update_synced_track_file_id(&conn, track_id, file_id).context("sqlite update_synced_track_file_id")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE synced_tracks SET file_id = $2 WHERE id = $1")
                    .bind(track_id)
                    .bind(file_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres update_synced_track_file_id")?;
                Ok(())
            }
        }
    }

    pub async fn update_synced_track_status(
        &self,
        track_id: i64,
        status: &str,
        resolved_url: Option<&str>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_synced_track_status connection")?;
                db::update_synced_track_status(&conn, track_id, status, resolved_url)
                    .context("sqlite update_synced_track_status")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE synced_tracks
                     SET import_status = $2, resolved_url = $3
                     WHERE id = $1",
                )
                .bind(track_id)
                .bind(status)
                .bind(resolved_url)
                .execute(pg_pool)
                .await
                .context("postgres update_synced_track_status")?;
                Ok(())
            }
        }
    }

    pub async fn delete_synced_tracks(&self, playlist_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_synced_tracks connection")?;
                db::delete_synced_tracks(&conn, playlist_id).context("sqlite delete_synced_tracks")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM synced_tracks WHERE playlist_id = $1")
                    .bind(playlist_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_synced_tracks")?;
                Ok(())
            }
        }
    }

    pub async fn count_synced_tracks(&self, playlist_id: i64) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_synced_tracks connection")?;
                db::count_synced_tracks(&conn, playlist_id).context("sqlite count_synced_tracks")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT COUNT(*)::BIGINT AS count
                     FROM synced_tracks
                     WHERE playlist_id = $1",
                )
                .bind(playlist_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres count_synced_tracks")?;
                Ok(row.get("count"))
            }
        }
    }
}

fn map_pg_synced_playlist(row: sqlx::postgres::PgRow) -> Result<SyncedPlaylist> {
    Ok(SyncedPlaylist {
        id: row.get("id"),
        user_id: row.get("user_id"),
        name: row.get("name"),
        description: row.get("description"),
        source_url: row.get("source_url"),
        source_platform: row.get("source_platform"),
        track_count: row.get("track_count"),
        matched_count: row.get("matched_count"),
        not_found_count: row.get("not_found_count"),
        sync_enabled: row.get::<i32, _>("sync_enabled") != 0,
        last_synced_at: row.get("last_synced_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn map_pg_synced_track(row: sqlx::postgres::PgRow) -> Result<SyncedTrack> {
    Ok(SyncedTrack {
        id: row.get("id"),
        playlist_id: row.get("playlist_id"),
        position: row.get("position"),
        title: row.get("title"),
        artist: row.get("artist"),
        duration_secs: row.get("duration_secs"),
        external_id: row.get("external_id"),
        source_url: row.get("source_url"),
        resolved_url: row.get("resolved_url"),
        import_status: row.get("import_status"),
        file_id: row.get("file_id"),
        added_at: row.get("added_at"),
    })
}
