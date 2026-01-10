use crate::database::value::PostcardValue;
use base64::{Engine as _, engine::general_purpose};
use sqlx::{
    Column, Row, TypeInfo,
    postgres::{PgColumn, PgRow},
};

pub fn convert_row_val_to_postcard(
    row_val: &PgRow,
    column: &PgColumn,
    index: usize,
) -> PostcardValue {
    // Types are taken from here: https://docs.rs/sqlx/latest/sqlx/postgres/types/index.html
    return match column.type_info().name() {
        "BOOL" => {
            let val: bool = row_val.get(index);
            PostcardValue::Bool(val)
        }
        "“CHAR”" => {
            let val: i8 = row_val.get(index);
            PostcardValue::Integer8(val)
        }
        "SMALLINT" | "SMALLSERIAL" | "INT2" => {
            let val: i16 = row_val.get(index);
            PostcardValue::Integer16(val)
        }
        "INT" | "SERIAL" | "INT4" => {
            let val: i32 = row_val.get(index);
            PostcardValue::Integer32(val)
        }
        "INT8" | "BIGSERIAL" | "BIGINT" => {
            let val: i64 = row_val.get(index);
            PostcardValue::Integer64(val)
        }
        "REAL" | "FLOAT4" => {
            let val: f32 = row_val.get(index);
            PostcardValue::Float32(val)
        }
        "DOUBLE PRECISION" | "FLOAT8" => {
            let val: f64 = row_val.get(index);
            PostcardValue::Float64(val)
        }
        "VARCHAR" | "CHAR(N)" | "TEXT" | "NAME" | "CITEXT" => {
            let val: String = row_val.get(index);
            PostcardValue::String(val)
        }
        "BYTEA" => {
            let val: Vec<u8> = row_val.get(index);
            PostcardValue::String(general_purpose::STANDARD.encode(val))
        }
        "VOID" => PostcardValue::Null,

        "TIMESTAMP" => {
            let val: time::PrimitiveDateTime = row_val.get(index);
            PostcardValue::String(val.to_string())
        }
        "TIMESTAMPTZ" => {
            let val: time::OffsetDateTime = row_val.get(index);
            PostcardValue::String(val.to_string())
        }
        "DATE" => {
            let val: time::Date = row_val.get(index);
            PostcardValue::String(val.to_string())
        }
        "TIME" => {
            let val: time::Time = row_val.get(index);
            PostcardValue::String(val.to_string())
        }
        "NUMERIC" => {
            let val: rust_decimal::Decimal = row_val.get(index);
            PostcardValue::String(val.to_string())
        }
        "UUID" => {
            let val: uuid::Uuid = row_val.get(index);
            PostcardValue::String(val.to_string())
        }
        _ => PostcardValue::Null,
    };
}
