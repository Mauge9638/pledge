use std::{sync::Arc, time::Instant};

use crate::QueryMatcher;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: Arc<PgPool>,
    pub matcher: Arc<QueryMatcher>,
    pub cache: Arc<moka::sync::Cache<String, (Vec<u8>, Instant)>>, // First value (Vec<u8>) is the serialized response, second value (Instant) is the expiration time
    pub global_ttl: u64,
}
