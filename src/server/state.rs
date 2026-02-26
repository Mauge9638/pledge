use std::sync::Arc;

use crate::{QueryMatcher, cache::lfu::Cache};
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: Arc<PgPool>,
    pub matcher: Arc<QueryMatcher>,
    pub cache: Arc<Cache>,
    pub global_ttl: u64,
}
