use serde::Deserialize;

pub mod lru;
pub mod matcher;
pub mod store;

#[derive(Debug, Deserialize, Clone)]
pub struct QueryTemplate {
    pub name: String,
    pub sql: String,
    pub ttl: Option<u64>,
}
