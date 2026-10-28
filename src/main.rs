//! Supply chain, logistics, warehouse and inventory management API.

/// Fetch a row by primary key or return a 404 `AppError`.
macro_rules! get_or_404 {
    ($entity:path, $db:expr, $id:expr, $what:expr) => {
        <$entity as sea_orm::EntityTrait>::find_by_id($id)
            .one($db)
            .await?
            .ok_or_else(|| crate::error::AppError::not_found($what))?
    };
}

mod entities;
mod error;
mod migration;
mod risk;
mod routes;
mod state;
mod stats;
mod stock;

use std::{env, sync::Arc, time::Duration};

use anyhow::Context;
use sea_orm::{ConnectOptions, Database};
use sea_orm_migration::MigratorTrait;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,tower_http=info")),
        )
        .init();

    let database_url = env::var("DATABASE_URL").context("DATABASE_URL must be set")?;
    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let run_migrations = env::var("RUN_MIGRATIONS").map(|v| v != "false" && v != "0").unwrap_or(true);
    let api_key = env::var("API_KEY").ok().filter(|k| !k.is_empty()).map(|k| Arc::<str>::from(k.as_str()));

    let mut opts = ConnectOptions::new(database_url);
    opts.max_connections(20)
        .min_connections(2)
        .connect_timeout(Duration::from_secs(8))
        .sqlx_logging(false);
    let db = Database::connect(opts).await.context("connecting to PostgreSQL")?;

    if run_migrations {
        migration::Migrator::up(&db, None).await.context("running migrations")?;
        tracing::info!("migrations applied");
    }
    if api_key.is_none() {
        tracing::warn!("API_KEY is not set: the API is unauthenticated");
    }

    let app = routes::router(state::AppState { db, api_key });
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("binding {bind_addr}"))?;
    tracing::info!("listening on {bind_addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await?;
    Ok(())
}
