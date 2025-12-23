use axum::{
    Json, Router,
    routing::{get, post},
};
use axum_server::tls_rustls::RustlsConfig;
use base64::{Engine as _, engine::general_purpose};
use serde::{Deserialize, Serialize};
use sqlx::{
    Column, PgPool, Row, TypeInfo,
    postgres::{PgColumn, PgPoolOptions, PgRow},
};
use std::{collections::HashMap, fs, net::SocketAddr, path::PathBuf, sync::Arc};
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

#[derive(Debug, Deserialize, Clone)]
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

struct QueryMatcher {
    templates: HashMap<String, QueryTemplate>,
}

#[derive(Clone)]
struct AppState {
    pool: Arc<PgPool>,
    matcher: Arc<QueryMatcher>,
}

impl QueryMatcher {
    fn new(config: &Config) -> Self {
        let mut templates = HashMap::new();
        for query in &config.queries {
            templates.insert(query.sql.clone(), query.clone());
        }
        QueryMatcher { templates }
    }

    fn find_template(&self, sql: &str) -> Option<&QueryTemplate> {
        self.templates.get(sql)
    }
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

    let matcher = QueryMatcher::new(&config);

    let state = AppState {
        pool: pool,
        matcher: Arc::new(matcher),
    };

    // println!(
    //     "Connected to the database!, with current connections: {}",
    //     pool.size()
    // );

    run_server(&config.server, state).await;
}

fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let contents = fs::read_to_string("pledge.toml")?;
    let config: Config = toml::from_str(&contents)?;
    Ok(config)
}

fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(|| async { "Hello, World!" }))
        .route("/health", get(health_handler))
        .route("/query", post(query_handler))
        .with_state(state)
}

async fn run_server(server_config: &ServerConfig, state: AppState) {
    let routes = create_router(state);
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
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(body): Json<QueryRequest>,
) -> Result<Json<QueryResponse>, (axum::http::StatusCode, String)> {
    println!("Received query: {:}", body.sql);
    println!("Params: {:?}", body.params);

    let mut query = sqlx::query(&body.sql);

    match state.matcher.find_template(&body.sql) {
        Some(template) => println!("✓ Matched template: {}", template.name),
        None => println!("✗ Query not predefined: {}", body.sql),
    };

    for param in &body.params {
        // Bind each parameter
        if let Some(num) = param.as_i64() {
            query = query.bind(num);
        } else if let Some(text) = param.as_str() {
            query = query.bind(text);
        } else if let Some(bool) = param.as_bool() {
            query = query.bind(bool);
        } else if let Some(array) = param.as_array() {
            query = query.bind(array);
        } else if let Some(num) = param.as_f64() {
            query = query.bind(num);
        } else {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                "Unsupported parameter type".to_string(),
            ));
        }
    }

    let rows = query
        .fetch_all(state.pool.as_ref())
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let rows_to_return: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|row| {
            let mut row_map = serde_json::Map::new();
            for (i, column) in row.columns().iter().enumerate() {
                let value = convert_row_val_to_serde(&row, column, i);

                row_map.insert(column.name().to_string(), value);
            }
            serde_json::Value::Object(row_map)
        })
        .collect();

    Ok(Json(QueryResponse {
        rows: rows_to_return,
    }))
}

fn convert_row_val_to_serde(row_val: &PgRow, column: &PgColumn, index: usize) -> serde_json::Value {
    // Types are taken from here: https://docs.rs/sqlx/latest/sqlx/postgres/types/index.html
    return match column.type_info().name() {
        "BOOL" => {
            let val: bool = row_val.get(index);
            serde_json::Value::Bool(val)
        }
        "“CHAR”" => {
            let val: i8 = row_val.get(index);
            serde_json::Value::Number(val.into())
        }
        "SMALLINT" | "SMALLSERIAL" | "INT2" => {
            let val: i16 = row_val.get(index);
            serde_json::Value::Number(val.into())
        }
        "INT" | "SERIAL" | "INT4" => {
            let val: i32 = row_val.get(index);
            serde_json::Value::Number(val.into())
        }
        "INT8" | "BIGSERIAL" | "BIGINT" => {
            let val: i64 = row_val.get(index);
            serde_json::Value::Number(val.into())
        }
        "REAL" | "FLOAT4" => {
            let val: f32 = row_val.get(index);
            let val_as_f64_option = serde_json::Number::from_f64(val.into());

            match val_as_f64_option {
                Some(val) => serde_json::Value::Number(val),
                None => serde_json::Value::Null,
            }
        }
        "DOUBLE PRECISION" | "FLOAT8" => {
            let val: f64 = row_val.get(index);
            let val_as_f64_option = serde_json::Number::from_f64(val);

            match val_as_f64_option {
                Some(val) => serde_json::Value::Number(val),
                None => serde_json::Value::Null,
            }
        }
        "VARCHAR" | "CHAR(N)" | "TEXT" | "NAME" | "CITEXT" => {
            let val: String = row_val.get(index);
            serde_json::Value::String(val)
        }
        "BYTEA" => {
            let val: Vec<u8> = row_val.get(index);
            serde_json::Value::String(general_purpose::STANDARD.encode(val))
        }
        "VOID" => serde_json::Value::Null,

        "TIMESTAMP" => {
            let val: time::PrimitiveDateTime = row_val.get(index);
            serde_json::Value::String(val.to_string())
        }
        "TIMESTAMPTZ" => {
            let val: time::OffsetDateTime = row_val.get(index);
            serde_json::Value::String(val.to_string())
        }
        "DATE" => {
            let val: time::Date = row_val.get(index);
            serde_json::Value::String(val.to_string())
        }
        "TIME" => {
            let val: time::Time = row_val.get(index);
            serde_json::Value::String(val.to_string())
        }
        "NUMERIC" => {
            let val: rust_decimal::Decimal = row_val.get(index);
            serde_json::Value::String(val.to_string())
        }
        "UUID" => {
            let val: uuid::Uuid = row_val.get(index);
            serde_json::Value::String(val.to_string())
        }
        _ => serde_json::Value::Null,
    };
}
