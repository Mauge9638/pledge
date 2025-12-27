use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;

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
    let state = AppState { pool, matcher };

    server::run_server(&config.server, state).await;
}
