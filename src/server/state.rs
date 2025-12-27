use std::sync::Arc;

use crate::QueryMatcher;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: Arc<PgPool>,
    pub matcher: Arc<QueryMatcher>,
}
