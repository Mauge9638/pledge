use std::{collections::HashMap, fmt::Display, string::FromUtf8Error};

use bytes::{BufMut, BytesMut};
use sqlx::{
    Column, Row,
    postgres::{PgColumn, PgRow},
};

use super::SQLCommand;

/*
 * Docs to look at for this
 * https://www.postgresql.org/docs/current/protocol-flow.html#PROTOCOL-FLOW-SIMPLE-QUERY
 * https://www.postgresql.org/docs/current/protocol-message-formats.html
 */

pub(super) trait Encode {
    fn encode(&self) -> Vec<u8>;
}

pub(super) trait Decode {
    fn decode(&self) -> Result<String, FromUtf8Error>;
}

pub(super) struct AuthenticationOk;
impl Encode for AuthenticationOk {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = BytesMut::with_capacity(9);
        bytes.put_u8(b'R');
        bytes.put_i32(8);
        bytes.put_i32(0);
        bytes.to_vec()
    }
}

pub(super) struct ReadyForQuery {
    pub(super) status: u8,
}
impl Encode for ReadyForQuery {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = BytesMut::with_capacity(6);
        bytes.put_u8(b'Z');
        bytes.put_i32(5);
        bytes.put_u8(self.status);
        bytes.to_vec()
    }
}

pub(super) struct RowDescription<'a> {
    pub(super) columns: &'a [PgColumn],
    pub(super) type_lens: &'a HashMap<u32, i16>,
}
impl Encode for RowDescription<'_> {
    fn encode(&self) -> Vec<u8> {
        let mut payload = BytesMut::new();
        for column in self.columns {
            let table_oid = match column.relation_id() {
                Some(oid) => oid.0 as i32,
                None => 0,
            };
            let attribute_number = match column.relation_attribute_no() {
                Some(number) => number as i16,
                None => 0,
            };
            let type_oid = match column.type_info().oid() {
                Some(oid) => oid.0 as i32,
                None => 0,
            };
            let type_len = match self.type_lens.get(&(type_oid as u32)) {
                Some(&type_len) => type_len,
                None => -2,
            };
            payload.put(column.name().as_bytes()); // Name
            payload.put_u8(0); // null terminator
            payload.put_i32(table_oid); // table OID
            payload.put_i16(attribute_number); // attribute number
            payload.put_i32(type_oid); // type OID
            payload.put_i16(type_len); // type size, this should be improved upon
            payload.put_i32(-1); //type modifier, -1 is a temporary workaround
            payload.put_i16(0); // format code, 0 = text, 1 = binary
        }
        let mut bytes = BytesMut::new();
        bytes.put_u8(b'T');
        bytes.put_i32((payload.len() + 6) as i32); // Length of message
        bytes.put_i16(self.columns.len() as i16); // Number of columns
        bytes.put(payload);
        bytes.to_vec()
    }
}
pub(super) struct DataRow<'a> {
    pub(super) row: &'a PgRow,
}
impl Encode for DataRow<'_> {
    fn encode(&self) -> Vec<u8> {
        let mut payload = BytesMut::new();
        for index in 0..(self.row.columns().len()) {
            let row_value = match self.row.try_get_raw(index) {
                Ok(pg_value_ref) => match pg_value_ref.as_bytes() {
                    Ok(value) => Some(value),
                    Err(_) => None,
                },
                Err(_) => None,
            };
            match row_value {
                Some(row_value) => {
                    payload.put_i32(row_value.len() as i32);
                    payload.put(row_value);
                }
                None => payload.put_i32(-1),
            }
        }

        let mut bytes = BytesMut::new();
        bytes.put_u8(b'D');
        bytes.put_i32((payload.len() + 6) as i32); // Length of message
        bytes.put_i16(self.row.len() as i16);
        bytes.put(payload);
        bytes.to_vec()
    }
}

pub(super) struct CommandComplete<'a> {
    pub(super) rows: u16,
    pub(super) command_tag: &'a SQLCommand,
}
impl Encode for CommandComplete<'_> {
    fn encode(&self) -> Vec<u8> {
        let mut payload = BytesMut::new();
        let command_tag = self.create_command_tag();
        payload.put(command_tag.as_bytes());
        payload.put_u8(0); // null terminator

        let mut bytes = BytesMut::new();
        bytes.put_u8(b'C');
        bytes.put_i32((payload.len() + 4) as i32); // Length of message
        bytes.put(payload);
        bytes.to_vec()
    }
}

impl CommandComplete<'_> {
    fn create_command_tag(&self) -> String {
        match self.command_tag {
            SQLCommand::Insert => format!("INSERT 0 {}", self.rows),
            SQLCommand::Delete => format!("DELETE {}", self.rows),
            SQLCommand::Update => format!("UPDATE {}", self.rows),
            SQLCommand::Merge => format!("MERGE {}", self.rows),
            SQLCommand::Select => format!("SELECT {}", self.rows),
            SQLCommand::CreateTableAs => format!("SELECT {}", self.rows),
            SQLCommand::Move => format!("MOVE {}", self.rows),
            SQLCommand::Fetch => format!("FETCH {}", self.rows),
            SQLCommand::Copy => format!("COPY {}", self.rows),
        }
    }
}

pub(super) enum ErrorResponseSeverity {
    Error,
    Fatal,
    Panic,
    Warning,
    Notice,
    Debug,
    Info,
    Log,
}

impl Display for ErrorResponseSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorResponseSeverity::Error => write!(f, "ERROR"),
            ErrorResponseSeverity::Fatal => write!(f, "FATAL"),
            ErrorResponseSeverity::Panic => write!(f, "PANIC"),
            ErrorResponseSeverity::Warning => write!(f, "WARNING"),
            ErrorResponseSeverity::Notice => write!(f, "NOTICE"),
            ErrorResponseSeverity::Debug => write!(f, "DEBUG"),
            ErrorResponseSeverity::Info => write!(f, "INFO"),
            ErrorResponseSeverity::Log => write!(f, "LOG"),
        }
    }
}

pub(super) struct ErrorResponse {
    pub(super) severity: ErrorResponseSeverity,
    pub(super) error_message: String,
    pub(super) sql_state_code: String,
}

impl Encode for ErrorResponse {
    fn encode(&self) -> Vec<u8> {
        let mut payload = BytesMut::new();
        payload.put_u8(b'S');
        payload.put(self.severity.to_string().as_bytes());
        payload.put_u8(0);
        payload.put_u8(b'V');
        payload.put(self.severity.to_string().as_bytes());
        payload.put_u8(0);
        payload.put_u8(b'C');
        payload.put(self.sql_state_code.as_bytes());
        payload.put_u8(0);
        payload.put_u8(b'M');
        payload.put(self.error_message.as_bytes());
        payload.put_u8(0);
        payload.put_u8(0); // Signals no more fields

        let mut bytes = BytesMut::new();
        bytes.put_u8(b'E');
        bytes.put_i32((payload.len() + 4) as i32); // Length of message
        bytes.put(payload);
        bytes.to_vec()
    }
}

// Client
pub(super) struct Query {
    pub(super) bytes: Vec<u8>,
}
impl Decode for Query {
    fn decode(&self) -> Result<String, FromUtf8Error> {
        let payload = self.bytes[5..self.bytes.len() - 1].to_vec();
        String::from_utf8(payload)
    }
}

// Helpers
