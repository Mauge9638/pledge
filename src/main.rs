use moka::sync::CacheBuilder;
use sqlx::postgres::PgPoolOptions;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

mod cache;
mod config;
mod database;
mod handlers;
mod server;
pub use cache::matcher::QueryMatcher;
pub use server::state::AppState;

#[tokio::main]
async fn main() {
    let config = config::load_config().expect("Failed to load config");
    let pool = Arc::new(
        PgPoolOptions::new()
            .max_connections(5)
            .connect(&config.database.url)
            .await
            .expect("Failed to connect to database"),
    );
    let matcher = Arc::new(QueryMatcher::new(&config));

    let max_ttl = config
        .queries
        .iter()
        .filter_map(|q| q.ttl)
        .max()
        .unwrap_or(config.cache.global_ttl)
        .max(config.cache.global_ttl);

    let cache = match config.cache.max_size_mb {
        Some(size) => {
            Arc::new(
                CacheBuilder::new(size * 1024 * 1024)
                    .weigher(|_key: &String, value: &(Vec<u8>, Instant)| {
                        value.0.len() as u32 // Weight by data size
                    })
                    .time_to_live(Duration::from_secs(max_ttl))
                    .build(),
            )
        }
        None => Arc::new(
            CacheBuilder::new(0)
                .weigher(|_key: &String, value: &(Vec<u8>, Instant)| {
                    value.0.len() as u32 // Weight by data size
                })
                .time_to_live(Duration::from_secs(max_ttl))
                .build(),
        ),
    };

    let state = AppState {
        pool,
        matcher,
        cache,
        global_ttl: config.cache.global_ttl,
    };

    server::run_server(&config.server, state).await;
}
