use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::core::config::{self, DatabaseDriver};
use crate::storage::db::DbPool;

mod pg_bootstrap;
mod types;

mod analytics;
mod content_subs;
mod download_history;
mod errors;
mod helpers;
mod playlists;
mod search;
mod sessions;
mod share_pages;
mod subscriptions;
mod synced_playlists;
mod task_queue;
mod uploads;
mod user_settings;
mod users;
mod vault;

pub use types::{ContentSourceGroup, ContentSubscriptionRecord, PreviewContext, QueueTaskInput, SharePageRecord};
pub use user_settings::VideoDownloadSettings;

use pg_bootstrap::POSTGRES_BOOTSTRAP_SQL;

#[derive(Clone)]
pub enum SharedStorage {
    Sqlite { db_pool: Arc<DbPool> },
    Postgres { sqlite_pool: Arc<DbPool>, pg_pool: PgPool },
}

impl SharedStorage {
    pub async fn from_sqlite_pool(db_pool: Arc<DbPool>) -> Result<Arc<Self>> {
        match *config::DATABASE_DRIVER {
            DatabaseDriver::Sqlite => Ok(Arc::new(Self::Sqlite { db_pool })),
            DatabaseDriver::Postgres => {
                let database_url = config::DATABASE_URL
                    .clone()
                    .ok_or_else(|| anyhow!("DATABASE_URL must be set when DATABASE_DRIVER=postgres"))?;
                let pg_pool = PgPoolOptions::new()
                    .max_connections(50)
                    .min_connections(5)
                    .acquire_timeout(Duration::from_secs(3))
                    .connect(&database_url)
                    .await
                    .context("connect postgres shared storage")?;
                sqlx::query(POSTGRES_BOOTSTRAP_SQL)
                    .execute(&pg_pool)
                    .await
                    .context("bootstrap postgres shared storage schema")?;
                Ok(Arc::new(Self::Postgres {
                    sqlite_pool: db_pool,
                    pg_pool,
                }))
            }
        }
    }

    pub fn sqlite_pool(&self) -> Arc<DbPool> {
        match self {
            Self::Sqlite { db_pool } => Arc::clone(db_pool),
            Self::Postgres { sqlite_pool, .. } => Arc::clone(sqlite_pool),
        }
    }

    pub fn is_postgres(&self) -> bool {
        matches!(self, Self::Postgres { .. })
    }
}
