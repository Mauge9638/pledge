use std::sync::Arc;

use crate::{QueryMatcher, cache::lru::Cache};
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState<const N: usize> {
    pub pool: Arc<PgPool>,
    pub matcher: Arc<QueryMatcher>,
    pub cache: Arc<Cache<N>>,
    pub global_ttl: u64,
}
