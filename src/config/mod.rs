use std::fs;

use crate::cache::QueryTemplate;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub database: DatabaseConfig,
    pub queries: Vec<QueryTemplate>,
    pub cache: CacheConfig,
    pub server: ServerConfig,
}

#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct CacheConfig {
    pub global_ttl: u64,
    pub max_size_mib: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    pub https_port: Option<u16>,
    pub tls_cert_path: Option<String>,
    pub tls_key_path: Option<String>,
}

pub fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let contents = fs::read_to_string("pledge.toml")?;
    let config: Config = toml::from_str(&contents)?;
    Ok(config)
}
