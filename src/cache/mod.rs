use serde::Deserialize;

pub mod lfu;
pub mod matcher;
pub mod store;
#[cfg(test)]
mod test;

#[derive(Debug, Deserialize, Clone)]
pub struct QueryTemplate {
    pub name: String,
    pub sql: String,
    pub ttl: Option<u64>,
}
