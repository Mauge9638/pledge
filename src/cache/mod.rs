use serde::Deserialize;

pub mod matcher;

#[derive(Debug, Deserialize, Clone)]
pub struct QueryTemplate {
    pub name: String,
    pub sql: String,
    pub ttl: Option<u64>,
}
