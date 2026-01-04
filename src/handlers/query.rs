use std::time::{Duration, Instant};

use crate::server::state::AppState;
use crate::{cache::store::cache_key, database::conversion};
use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use sqlx::{Column, PgPool, Row};

#[derive(Deserialize)]
pub struct QueryRequest {
    sql: String,
    params: Vec<serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
pub struct QueryResponse {
    rows: Vec<serde_json::Value>,
}

pub async fn query_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(body): Json<QueryRequest>,
) -> Result<Response, (StatusCode, String)> {
    let handler_start = Instant::now();
    println!("Received query: {:}", body.sql);
    println!("Params: {:?}", body.params);

    let matched_template = state.matcher.find_template(&body.sql);
    let key = cache_key(&body.sql, &body.params);

    if matched_template.is_some() {
        if let Some((cached_result, expiry)) = state.cache.get(&key) {
            if Instant::now() < expiry {
                let cache_get_time = handler_start.elapsed();
                println!("Cache hit - get time: {:?}", cache_get_time);

                let copy_start = Instant::now();
                let bytes = cached_result.to_vec();
                println!("Copy time: {:?}", copy_start.elapsed());

                let response = Response::builder()
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(bytes.into())
                    .unwrap();

                println!("Total cache hit time: {:?}", handler_start.elapsed());
                return Ok(response);
            } else {
                println!(
                    "Cache expired per entry level TTL, invalidating and retrieving new value"
                );
                state.cache.invalidate(&key);
            }
        }
    }

    // Cache miss path
    let db_start = Instant::now();
    let rows = execute_query(&state.pool, &body.sql, &body.params).await?;
    println!("DB query time: {:?}", db_start.elapsed());
    println!("Rows returned: {}", rows.len());

    let serialize_start = Instant::now();
    let response = QueryResponse { rows };
    let serialized = serde_json::to_vec(&response)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    println!("Serialization time: {:?}", serialize_start.elapsed());
    println!(
        "Response size: {} bytes ({:.2} MB)",
        serialized.len(),
        serialized.len() as f64 / 1_000_000.0
    );

    match matched_template {
        Some(template) => {
            let cache_insert_start = Instant::now();
            let expiration = match template.ttl {
                Some(ttl) => Instant::now() + Duration::from_secs(ttl),
                None => Instant::now() + Duration::from_secs(state.global_ttl),
            };

            state.cache.insert(key, (serialized.clone(), expiration));
            println!("Cache insert time: {:?}", cache_insert_start.elapsed());
        }
        None => {}
    }

    println!("Total handler time: {:?}", handler_start.elapsed());

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .body(serialized.into())
        .unwrap())
}

async fn execute_query(
    pool: &PgPool,
    sql: &str,
    params: &[serde_json::Value],
) -> Result<Vec<serde_json::Value>, (axum::http::StatusCode, String)> {
    let mut query = sqlx::query(sql);

    for param in params {
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
        .fetch_all(pool)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let rows_to_return: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|row| {
            let mut row_map = serde_json::Map::new();
            for (i, column) in row.columns().iter().enumerate() {
                let value = conversion::convert_row_val_to_serde(&row, column, i);

                row_map.insert(column.name().to_string(), value);
            }
            serde_json::Value::Object(row_map)
        })
        .collect();

    Ok(rows_to_return)
}
