use axum::{
    Json, Router,
    routing::{get, post},
};
use axum_server::tls_rustls::RustlsConfig;
use serde::{Deserialize, Serialize};
use sqlx::{Column, PgPool, Row, TypeInfo, postgres::PgPoolOptions};
use std::{fs, net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::task::JoinHandle;

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

#[derive(Deserialize)]
struct QueryRequest {
    sql: String,
    params: Vec<serde_json::Value>,
}

#[derive(Serialize)]
struct QueryResponse {
    rows: Vec<serde_json::Value>,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
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
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database.url)
        .await
        .expect("Failed to connect to database");

    let pool = Arc::new(pool);

    println!(
        "Connected to the database!, with current connections: {}",
        pool.size()
    );

    run_server(&config.server, pool).await;
}

fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let contents = fs::read_to_string("pledge.toml")?;
    let config: Config = toml::from_str(&contents)?;
    Ok(config)
}

fn create_router(pool: Arc<PgPool>) -> Router {
    Router::new()
        .route("/", get(|| async { "Hello, World!" }))
        .route("/health", get(health_handler))
        .route("/query", post(query_handler))
        .with_state(pool)
}

async fn run_server(server_config: &ServerConfig, pool: Arc<PgPool>) {
    let routes = create_router(pool);
    let cloned_routes = routes.clone();
    let port = server_config.port;

    let http_handle = tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await {
            Ok(listener) => {
                println!("Server listening on HTTP on port {}", port);
                listener
            }
            Err(err) => {
                eprintln!("Failed to bind to port {}: {}", port, err);
                return;
            }
        };

        match axum::serve(listener, routes).await {
            Ok(_) => {}
            Err(err) => {
                eprintln!("Failed to serve HTTP server: {}", err);
            }
        };
    });

    let https_handle: Option<JoinHandle<()>> = if let (Some(https_port), Some(cert), Some(key)) = (
        server_config.https_port,
        server_config.tls_cert_path.clone(),
        server_config.tls_key_path.clone(),
    ) && https_port != port
    {
        Some(tokio::spawn(async move {
            let config =
                match RustlsConfig::from_pem_file(PathBuf::from(cert), PathBuf::from(key)).await {
                    Ok(config) => {
                        println!("Server listening on HTTPS on port {}", https_port);
                        config
                    }
                    Err(err) => {
                        eprintln!("Failed to load TLS certificate and key: {}", err);
                        return;
                    }
                };

            let addr = SocketAddr::from(([0, 0, 0, 0], https_port));
            match axum_server::bind_rustls(addr, config)
                .serve(cloned_routes.into_make_service())
                .await
            {
                Ok(_) => {}
                Err(err) => eprintln!("Failed to start HTTPS server: {}", err),
            };
        }))
    } else {
        None
    };

    match https_handle {
        Some(https_handle) => {
            let (http, https) = tokio::join!(http_handle, https_handle);
            match http {
                Ok(_) => {}
                Err(err) => eprintln!("{}", err),
            };
            match https {
                Ok(_) => {}
                Err(err) => eprintln!("{}", err),
            };
        }
        None => {
            let _ = http_handle.await;
        }
    }
}

async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "OK".to_string(),
    })
}

async fn query_handler(
    axum::extract::State(pool): axum::extract::State<Arc<PgPool>>,
    Json(body): Json<QueryRequest>,
) -> Result<Json<QueryResponse>, (axum::http::StatusCode, String)> {
    println!("Received query: {:}", body.sql);
    println!("Params: {:?}", body.params);

    // let fake_row = serde_json::json!({
    //     "id": body.params.get(0).unwrap_or(&serde_json::json!(1)),
    //     "name": "Magnus",
    //     "email": "example@mail.com"
    // });

    let mut query = sqlx::query(&body.sql);

    for param in &body.params {
        // Bind each parameter
        if let Some(num) = param.as_i64() {
            query = query.bind(num);
        } else if let Some(text) = param.as_str() {
            query = query.bind(text);
        }
        // Add more types as needed
    }

    let rows = query
        .fetch_all(pool.as_ref())
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let rows_to_return: Vec<serde_json::Map<String, serde_json::Value>> = rows
        .into_iter()
        .map(|row| {
            let mut row_map = serde_json::Map::new();
            for (i, column) in row.columns().iter().enumerate() {
                let value = match column.type_info().name() {
                    "INT4" => {
                        let val: i32 = row.get(i);
                        serde_json::Value::Number(val.into())
                    }
                    "INT8" => {
                        let val: i64 = row.get(i);
                        serde_json::Value::Number(val.into())
                    }
                    "TEXT" | "VARCHAR" => {
                        let val: String = row.get(i);
                        serde_json::Value::String(val.into())
                    }
                    "BOOL" => {
                        let val: bool = row.get(i);
                        serde_json::Value::Bool(val.into())
                    }
                    // "FLOAT4" => {
                    //     let val: f32 = row.get(i);
                    //     serde_json::Value::Number(val.into())
                    // }
                    // "FLOAT8" => {
                    //     let val: f64 = row.get(i);
                    //     serde_json::Value::Number(val.into())
                    // }
                    _ => serde_json::Value::Null,
                };
                row_map.insert(column.name().to_string(), value);
            }
            row_map
        })
        .collect();

    // let result = serde_json::json!({
    //     "status": "OK",
    //     "data": rowsToReturn
    // });

    Ok(Json(QueryResponse {
        rows: vec![serde_json::json!(rows_to_return)],
    }))
}
