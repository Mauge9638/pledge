use bytes::{BufMut, BytesMut};
use sqlx::{
    Column, Row, TypeInfo,
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
    fn decode(&self) -> String;
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
            payload.put(column.name().as_bytes()); // Name
            payload.put_u8(0); // null terminator
            payload.put_i32(table_oid); // table OID
            payload.put_i16(attribute_number); // attribute number
            payload.put_i32(type_oid); // type OID
            payload.put_i16(type_name_to_size(column.type_info().name())); // type size, this should be improved upon
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

pub(super) struct ErrorResponse {
    pub(super) error_message: String,
    pub(super) sql_state_code: String,
}

impl Encode for ErrorResponse {
    fn encode(&self) -> Vec<u8> {
        let mut payload = BytesMut::new();

        let mut bytes = BytesMut::new();
        bytes.put_u8(b'E');

        bytes.to_vec()
    }
}

// Client
pub(super) struct Query {
    pub(super) bytes: Vec<u8>,
}
impl Decode for Query {
    fn decode(&self) -> String {
        let payload = self.bytes[5..self.bytes.len() - 1].to_vec();
        String::from_utf8(payload).unwrap()
    }
}

// Helpers

fn type_name_to_size(type_name: &str) -> i16 {
    match type_name.to_lowercase().as_str() {
        "int4" => 4,
        "text" => -1,
        "varchar" => -1,
        _ => 1,
    }
}
