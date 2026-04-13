use std::{collections::HashMap, fmt::Display, string::FromUtf8Error};

use super::reader::{ByteReader, ByteReaderError};
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
    type Output;
    fn decode(&self) -> Result<Self::Output, ByteReaderError>;
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
impl<'a> Encode for RowDescription<'a> {
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
impl<'a> Encode for DataRow<'a> {
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
impl<'a> Encode for CommandComplete<'a> {
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

impl<'a> CommandComplete<'a> {
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

pub(super) enum ClientMessageContent {
    QueryMessage(QueryMessageContent),
    ParseMessage(ParseMessageContent),
    //BindMessage(Bind),
    //ExecuteMessage(Execute),
    //SyncMessage(Sync),
    //DescribeMessage(Describe),
    //CloseMessage(Close),
    //TerminateMessage(Terminate),
}

// Client
pub(super) struct QueryMessageContent {
    pub(super) query: String,
}
pub(super) struct Query {
    pub(super) bytes: Vec<u8>,
}
impl Decode for Query {
    type Output = QueryMessageContent;
    fn decode(&self) -> Result<QueryMessageContent, ByteReaderError> {
        Ok(QueryMessageContent {
            query: ByteReader::new(&self.bytes, 0).read_cstring()?,
        })
    }
}

pub(super) struct ParseMessageContent {
    pub(super) name: String,
    pub(super) query: String,
    pub(super) parameter_data_types_len: i16,
    pub(super) parameter_data_types: Vec<i32>,
}
pub(super) struct Parse {
    pub(super) bytes: Vec<u8>,
}
impl Decode for Parse {
    type Output = ParseMessageContent;
    fn decode(&self) -> Result<ParseMessageContent, ByteReaderError> {
        let mut reader = ByteReader::new(&self.bytes, 0);
        let mut parsed_message = ParseMessageContent {
            name: reader.read_cstring()?,
            query: reader.read_cstring()?,
            parameter_data_types_len: reader.read_i16()?,
            parameter_data_types: Vec::new(),
        };
        for _ in 0..parsed_message.parameter_data_types_len {
            parsed_message.parameter_data_types.push(reader.read_i32()?)
        }

        println!(
            "name: '{}', query: '{}', parameter_data_types_len: {}, parameter_data_types: {:?}",
            parsed_message.name,
            parsed_message.query,
            parsed_message.parameter_data_types_len,
            parsed_message.parameter_data_types
        );

        Ok(parsed_message)
    }
}

/*
* Bind (F)
* String The name of the destination portal (an empty string selects the unnamed portal).
* String The name of the source prepared statement (an empty string selects the unnamed prepared statement).
* Int16 The number of parameter format codes that follow (denoted C below). This can be zero to indicate that there are no parameters or that the parameters all use the default format (text); or one, in which case the specified format code is applied to all parameters; or it can equal the actual number of parameters.
* Int16[C] The parameter format codes. Each must presently be zero (text) or one (binary).
* Int16 The number of parameter values that follow (possibly zero). This must match the number of parameters needed by the query.
*
* Next, the following pair of fields appear for each parameter:
* Int32 The length of the parameter value, in bytes (this count does not include itself). Can be zero. As a special case, -1 indicates a NULL parameter value. No value bytes follow in the NULL case.
* Byte n The value of the parameter, in the format indicated by the associated format code. n is the above length.
*
* After the last parameter, the following fields appear:
* Int16 The number of result-column format codes that follow (denoted R below). This can be zero to indicate that there are no result columns or that the result columns should all use the default format (text); or one, in which case the specified format code is applied to all result columns (if any); or it can equal the actual number of result columns of the query.
* Int16[R] The result-column format codes. Each must presently be zero (text) or one (binary).
*/
pub(super) struct BindMessageContent {
    pub(super) name: String,
    pub(super) source_prepared_statement: String,
    pub(super) parameter_format_codes_len: i16,
    pub(super) parameter_format_codes: Vec<i16>,
    pub(super) parameter_values_len: i16,
    pub(super) parameter_values_byte_len: i32,
    pub(super) parameter_values: Vec<Vec<u8>>,
    pub(super) result_column_format_codes_len: i16,
    pub(super) result_column_format_codes: Vec<i16>,
}
pub(super) struct Bind {
    pub(super) bytes: Vec<u8>,
}
impl Decode for Bind {
    type Output = BindMessageContent;
    fn decode(&self) -> Result<BindMessageContent, ByteReaderError> {
        let mut reader = ByteReader::new(&self.bytes, 0);
        let mut parsed_message = BindMessageContent {
            name: reader.read_cstring()?,
            query: reader.read_cstring()?,
            parameter_data_types_len: reader.read_i16()?,
            parameter_data_types: Vec::new(),
        };
        for _ in 0..parsed_message.parameter_data_types_len {
            parsed_message.parameter_data_types.push(reader.read_i32()?)
        }

        println!(
            "name: '{}', query: '{}', parameter_data_types_len: {}, parameter_data_types: {:?}",
            parsed_message.name,
            parsed_message.query,
            parsed_message.parameter_data_types_len,
            parsed_message.parameter_data_types
        );

        Ok(parsed_message)
    }
}
// Helpers
