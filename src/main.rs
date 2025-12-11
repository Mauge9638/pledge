use axum::{
    Json, Router,
    routing::{get, post},
};
use axum_server::tls_rustls::RustlsConfig;
use serde::Deserialize;
use std::{fs, net::SocketAddr, path::PathBuf};

#[derive(Debug, Deserialize)]
struct Config {
    database: DatabaseConfig,
    queries: Vec<QueryTemplate>,
    cache: CacheConfig,
    server: ServerConfig,
}

#[derive(Debug, Deserialize)]
struct DatabaseConfig {
    url: String,
}

#[derive(Debug, Deserialize)]
struct QueryTemplate {
    name: String,
    sql: String,
    ttl: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct CacheConfig {
    global_ttl: u64,
}

#[derive(Debug, Deserialize)]
struct ServerConfig {
    port: u16,
    https_port: Option<u16>,
    tls_cert_path: Option<String>,
    tls_key_path: Option<String>,
}

#[tokio::main]
async fn main() {
    let config = load_config().expect("Failed to load config");
    println!("Database url: {}", config.database.url);
    println!("Cache global TTL: {}", config.cache.global_ttl);
    println!("Loaded {} queries", config.queries.len());
    for query in &config.queries {
        let name = &query.name;
        let sql = &query.sql;
        let ttl = &query.ttl.unwrap_or_default();
        println!(
            "  - {}: \n With SQL: {:?} \n With TTL: {:?}\n",
            name, sql, ttl
        );
    }
    run_server(&config.server).await;
}

fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let contents = fs::read_to_string("pledge.toml")?;
    let config: Config = toml::from_str(&contents)?;
    Ok(config)
}

fn create_router() -> Router {
    return Router::new()
        .route("/", get(|| async { "Hello, World!" }))
        .route("/healthcheck", get("OK"))
        .route("/sql", post(sql_handler));
}

async fn run_server(server_config: &ServerConfig) {
    let routes = create_router();
    let port = &server_config.https_port.unwrap_or(server_config.port);
    let tls_cert_path = match &server_config.tls_cert_path {
        Some(cert) => cert,
        None => panic!("TLS certificate not provided"),
    };
    let tls_key_path = match &server_config.tls_key_path {
        Some(key) => key,
        None => panic!("TLS key not provided"),
    };
    println!("TLS key: {}", tls_key_path);
    println!("TLS certificate: {}", tls_cert_path);

    let config =
        RustlsConfig::from_pem_file(PathBuf::from(tls_cert_path), PathBuf::from(tls_key_path))
            .await
            .unwrap();

    // let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
    //     .await
    //     .unwrap();
    // axum::serve(listener, routes).await.unwrap();

    let addr = SocketAddr::from(([127, 0, 0, 1], port.clone()));
    axum_server::bind_rustls(addr, config)
        .serve(routes.into_make_service())
        .await
        .unwrap();
    println!("Server running on port: {}", port);
}

async fn sql_handler(Json(body): Json<serde_json::Value>) -> String {
    println!("Received body parameters: {:?}", body);
    let return_value = format!("Body handled successfully {:?}", body);
    return_value
}
