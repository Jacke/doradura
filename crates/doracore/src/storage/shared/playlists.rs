use anyhow::{Context, Result};
use sqlx::Row;

use crate::storage::db::{self, Playlist, PlaylistItem};

use super::SharedStorage;

impl SharedStorage {
    pub async fn create_playlist(&self, user_id: i64, name: &str, description: Option<&str>) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_playlist connection")?;
                db::create_playlist(&conn, user_id, name, description).context("sqlite create_playlist")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO playlists (user_id, name, description, created_at, updated_at)
                     VALUES ($1, $2, $3, NOW(), NOW())
                     RETURNING id",
                )
                .bind(user_id)
                .bind(name)
                .bind(description)
                .fetch_one(pg_pool)
                .await
                .context("postgres create_playlist")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn get_playlist(&self, playlist_id: i64) -> Result<Option<Playlist>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_playlist connection")?;
                db::get_playlist(&conn, playlist_id).context("sqlite get_playlist")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, user_id, name, description, is_public, share_token,
                            CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM playlists
                     WHERE id = $1",
                )
                .bind(playlist_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_playlist")?;
                row.map(map_pg_playlist).transpose()
            }
        }
    }

    pub async fn get_user_playlists(&self, user_id: i64) -> Result<Vec<Playlist>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_playlists connection")?;
                db::get_user_playlists(&conn, user_id).context("sqlite get_user_playlists")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, user_id, name, description, is_public, share_token,
                            CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM playlists
                     WHERE user_id = $1
                     ORDER BY updated_at DESC",
                )
                .bind(user_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_playlists")?;
                rows.into_iter().map(map_pg_playlist).collect()
            }
        }
    }

    pub async fn rename_playlist(&self, playlist_id: i64, user_id: i64, name: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite rename_playlist connection")?;
                db::rename_playlist(&conn, playlist_id, user_id, name).context("sqlite rename_playlist")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE playlists
                     SET name = $2, updated_at = NOW()
                     WHERE id = $1",
                )
                .bind(playlist_id)
                .bind(name)
                .execute(pg_pool)
                .await
                .context("postgres rename_playlist")?;
                Ok(())
            }
        }
    }

    pub async fn delete_playlist(&self, playlist_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_playlist connection")?;
                db::delete_playlist(&conn, playlist_id).context("sqlite delete_playlist")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM playlists WHERE id = $1")
                    .bind(playlist_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_playlist")?;
                Ok(())
            }
        }
    }

    pub async fn count_user_playlists(&self, user_id: i64) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_user_playlists connection")?;
                db::count_user_playlists(&conn, user_id).context("sqlite count_user_playlists")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT COUNT(*)::BIGINT AS count
                     FROM playlists
                     WHERE user_id = $1",
                )
                .bind(user_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres count_user_playlists")?;
                Ok(row.get("count"))
            }
        }
    }

    pub async fn set_playlist_share_token(&self, playlist_id: i64, token: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_playlist_share_token connection")?;
                db::set_playlist_share_token(&conn, playlist_id, token).context("sqlite set_playlist_share_token")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE playlists
                     SET share_token = $2, updated_at = NOW()
                     WHERE id = $1",
                )
                .bind(playlist_id)
                .bind(token)
                .execute(pg_pool)
                .await
                .context("postgres set_playlist_share_token")?;
                Ok(())
            }
        }
    }

    pub async fn set_playlist_public(&self, playlist_id: i64, is_public: bool) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_playlist_public connection")?;
                db::set_playlist_public(&conn, playlist_id, is_public).context("sqlite set_playlist_public")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE playlists
                     SET is_public = $2, updated_at = NOW()
                     WHERE id = $1",
                )
                .bind(playlist_id)
                .bind(if is_public { 1_i32 } else { 0_i32 })
                .execute(pg_pool)
                .await
                .context("postgres set_playlist_public")?;
                Ok(())
            }
        }
    }

    pub async fn get_playlist_by_share_token(&self, token: &str) -> Result<Option<Playlist>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_playlist_by_share_token connection")?;
                db::get_playlist_by_share_token(&conn, token).context("sqlite get_playlist_by_share_token")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, user_id, name, description, is_public, share_token,
                            CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM playlists
                     WHERE share_token = $1",
                )
                .bind(token)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_playlist_by_share_token")?;
                row.map(map_pg_playlist).transpose()
            }
        }
    }

    pub async fn add_playlist_item(
        &self,
        playlist_id: i64,
        title: &str,
        artist: Option<&str>,
        url: &str,
        duration_secs: Option<i32>,
        file_id: Option<&str>,
        source: &str,
    ) -> Result<i64> {
        // Reject non-video URLs (channels, playlists, user pages) — these hang yt-dlp
        if url.contains("/channel/") || url.contains("/playlist?") || url.contains("/user/") || url.contains("/@") {
            anyhow::bail!("Cannot add non-video URL to playlist: {}", url);
        }
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite add_playlist_item connection")?;
                db::add_playlist_item(&conn, playlist_id, title, artist, url, duration_secs, file_id, source)
                    .context("sqlite add_playlist_item")
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("postgres add_playlist_item begin")?;
                let inserted = sqlx::query(
                    "INSERT INTO playlist_items (
                        playlist_id, position, title, artist, url, duration_secs, file_id, source, added_at
                     ) VALUES (
                        $1,
                        (SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_items WHERE playlist_id = $1 FOR UPDATE),
                        $2, $3, $4, $5, $6, $7, NOW()
                     )
                     RETURNING id",
                )
                .bind(playlist_id)
                .bind(title)
                .bind(artist)
                .bind(url)
                .bind(duration_secs)
                .bind(file_id)
                .bind(source)
                .fetch_one(&mut *tx)
                .await
                .context("postgres add_playlist_item insert")?;
                sqlx::query("UPDATE playlists SET updated_at = NOW() WHERE id = $1")
                    .bind(playlist_id)
                    .execute(&mut *tx)
                    .await
                    .context("postgres add_playlist_item touch_playlist")?;
                tx.commit().await.context("postgres add_playlist_item commit")?;
                Ok(inserted.get("id"))
            }
        }
    }

    pub async fn remove_playlist_item(&self, item_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite remove_playlist_item connection")?;
                db::remove_playlist_item(&conn, item_id).context("sqlite remove_playlist_item")
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("postgres remove_playlist_item begin")?;
                let playlist_id = sqlx::query("SELECT playlist_id FROM playlist_items WHERE id = $1")
                    .bind(item_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .context("postgres remove_playlist_item select")?
                    .map(|row| row.get::<i64, _>("playlist_id"));
                sqlx::query("DELETE FROM playlist_items WHERE id = $1")
                    .bind(item_id)
                    .execute(&mut *tx)
                    .await
                    .context("postgres remove_playlist_item delete")?;
                if let Some(playlist_id) = playlist_id {
                    sqlx::query("UPDATE playlists SET updated_at = NOW() WHERE id = $1")
                        .bind(playlist_id)
                        .execute(&mut *tx)
                        .await
                        .context("postgres remove_playlist_item touch_playlist")?;
                }
                tx.commit().await.context("postgres remove_playlist_item commit")?;
                Ok(())
            }
        }
    }

    pub async fn reorder_playlist_item(&self, item_id: i64, direction: i32) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite reorder_playlist_item connection")?;
                db::reorder_playlist_item(&conn, item_id, direction).context("sqlite reorder_playlist_item")
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("postgres reorder_playlist_item begin")?;
                let row = sqlx::query(
                    "SELECT playlist_id, position
                     FROM playlist_items
                     WHERE id = $1",
                )
                .bind(item_id)
                .fetch_one(&mut *tx)
                .await
                .context("postgres reorder_playlist_item select")?;
                let playlist_id: i64 = row.get("playlist_id");
                let current_position: i32 = row.get("position");
                let new_position = current_position + direction;
                if new_position < 0 {
                    tx.commit()
                        .await
                        .context("postgres reorder_playlist_item noop commit")?;
                    return Ok(());
                }
                sqlx::query(
                    "UPDATE playlist_items
                     SET position = $1
                     WHERE playlist_id = $2 AND position = $3",
                )
                .bind(current_position)
                .bind(playlist_id)
                .bind(new_position)
                .execute(&mut *tx)
                .await
                .context("postgres reorder_playlist_item swap")?;
                sqlx::query("UPDATE playlist_items SET position = $1 WHERE id = $2")
                    .bind(new_position)
                    .bind(item_id)
                    .execute(&mut *tx)
                    .await
                    .context("postgres reorder_playlist_item set")?;
                sqlx::query("UPDATE playlists SET updated_at = NOW() WHERE id = $1")
                    .bind(playlist_id)
                    .execute(&mut *tx)
                    .await
                    .context("postgres reorder_playlist_item touch_playlist")?;
                tx.commit().await.context("postgres reorder_playlist_item commit")?;
                Ok(())
            }
        }
    }

    pub async fn get_playlist_items(&self, playlist_id: i64) -> Result<Vec<PlaylistItem>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_playlist_items connection")?;
                db::get_playlist_items(&conn, playlist_id).context("sqlite get_playlist_items")
            }
            Self::Postgres { pg_pool, .. } => sqlx::query_as::<_, PlaylistItem>(
                "SELECT id, playlist_id, position, download_history_id, title, artist, url,
                        duration_secs, file_id, source, CAST(added_at AS TEXT) AS added_at
                 FROM playlist_items
                 WHERE playlist_id = $1
                 ORDER BY position",
            )
            .bind(playlist_id)
            .fetch_all(pg_pool)
            .await
            .context("postgres get_playlist_items"),
        }
    }

    pub async fn get_playlist_items_page(
        &self,
        playlist_id: i64,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<PlaylistItem>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_playlist_items_page connection")?;
                db::get_playlist_items_page(&conn, playlist_id, offset, limit).context("sqlite get_playlist_items_page")
            }
            Self::Postgres { pg_pool, .. } => sqlx::query_as::<_, PlaylistItem>(
                "SELECT id, playlist_id, position, download_history_id, title, artist, url,
                        duration_secs, file_id, source, CAST(added_at AS TEXT) AS added_at
                 FROM playlist_items
                 WHERE playlist_id = $1
                 ORDER BY position
                 LIMIT $2 OFFSET $3",
            )
            .bind(playlist_id)
            .bind(limit)
            .bind(offset)
            .fetch_all(pg_pool)
            .await
            .context("postgres get_playlist_items_page"),
        }
    }

    pub async fn count_playlist_items(&self, playlist_id: i64) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_playlist_items connection")?;
                db::count_playlist_items(&conn, playlist_id).context("sqlite count_playlist_items")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT COUNT(*)::BIGINT AS count
                     FROM playlist_items
                     WHERE playlist_id = $1",
                )
                .bind(playlist_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres count_playlist_items")?;
                Ok(row.get("count"))
            }
        }
    }

    pub async fn update_playlist_item_file_id(&self, item_id: i64, file_id: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_playlist_item_file_id connection")?;
                db::update_item_file_id(&conn, item_id, file_id).context("sqlite update_playlist_item_file_id")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE playlist_items SET file_id = $2 WHERE id = $1")
                    .bind(item_id)
                    .bind(file_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres update_playlist_item_file_id")?;
                Ok(())
            }
        }
    }
}

fn map_pg_playlist(row: sqlx::postgres::PgRow) -> Result<Playlist> {
    Ok(Playlist {
        id: row.get("id"),
        user_id: row.get("user_id"),
        name: row.get("name"),
        description: row.get("description"),
        is_public: row.get::<i32, _>("is_public") != 0,
        share_token: row.get("share_token"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

// `map_pg_playlist_item` removed — replaced by `#[derive(sqlx::FromRow)]` on
// `PlaylistItem` + `sqlx::query_as::<_, PlaylistItem>` at call sites.
