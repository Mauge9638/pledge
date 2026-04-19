use std::{collections::HashMap, sync::Arc};

use crate::{QueryMatcher, cache::lfu::Cache, config::DatabaseConfig};
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: Arc<PgPool>,
    pub database_config: DatabaseConfig,
    pub matcher: Arc<QueryMatcher>,
    pub cache: Arc<Cache>,
    pub global_ttl: u64,
    // pub pg_type_lens: Arc<HashMap<u32, i16>>,
}
