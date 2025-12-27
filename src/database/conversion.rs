use base64::{Engine as _, engine::general_purpose};
use sqlx::{
    Column, Row, TypeInfo,
    postgres::{PgColumn, PgRow},
};

pub fn convert_row_val_to_serde(
    row_val: &PgRow,
    column: &PgColumn,
    index: usize,
) -> serde_json::Value {
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
