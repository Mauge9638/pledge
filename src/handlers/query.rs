use crate::database::conversion;
use crate::server::state::AppState;
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::{Column, Row};

#[derive(Deserialize)]
pub struct QueryRequest {
    sql: String,
    params: Vec<serde_json::Value>,
}

#[derive(Serialize)]
pub struct QueryResponse {
    rows: Vec<serde_json::Value>,
}

pub async fn query_handler(
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
                let value = conversion::convert_row_val_to_serde(&row, column, i);

                row_map.insert(column.name().to_string(), value);
            }
            serde_json::Value::Object(row_map)
        })
        .collect();

    Ok(Json(QueryResponse {
        rows: rows_to_return,
    }))
}
