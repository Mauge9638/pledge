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
mod metrics;
mod server;
pub use cache::matcher::QueryMatcher;
pub use server::state::AppState;

use crate::metrics::{CACHE_MEMORY_BYTES, CACHE_SIZE};

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

    let cache_size = match config.cache.max_size_mib {
        Some(size) => size * 1_024 * 1_024,
        None => 100 * 1_024 * 1_024, // Default to 100MiB cache size
    };

    let cache = Arc::new(
        CacheBuilder::new(cache_size)
            .weigher(|_key: &String, value: &(Vec<u8>, Instant)| {
                value.0.len() as u32 // Weight by data size
            })
            .time_to_live(Duration::from_secs(max_ttl))
            .build(),
    );

    println!("Cache initialized: {} MiB", cache_size / 1_024 / 1_024);
    {
        let sysinfo = sysinfo::System::new_all();
        let total_ram = sysinfo.total_memory();
        if (cache_size as f64) > (total_ram as f64 * 0.8) {
            // Using 80% of system RAM
            eprintln!(
                "WARNING: Cache size {}MiB is close to total system RAM {}MiB",
                cache_size / (1_024 * 1_024),
                total_ram / (1_024 * 1_024)
            );
            eprintln!("Consider reducing cache size or increasing system RAM");
        }
    }

    // Register metrics
    metrics::register_metrics();
    let cache_clone = cache.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            CACHE_SIZE.set(cache_clone.entry_count() as f64);
            CACHE_MEMORY_BYTES.set(cache_clone.weighted_size() as f64);
        }
    });

    let state = AppState {
        pool,
        matcher,
        cache,
        global_ttl: config.cache.global_ttl,
    };

    server::run_server(&config.server, state).await;
}
