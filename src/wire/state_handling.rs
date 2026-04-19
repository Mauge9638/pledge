use super::messages::{
    BindComplete, BindMessageContent,
    ClientMessageContent::{
        BindMessage, DescribeMessage, ExecuteMessage, ParseMessage, QueryMessage, SyncMessage,
        TerminateMessage,
    },
    DescribeMessageContent, ExecuteMessageContent, ParseComplete, ParseMessageContent,
    QueryMessageContent,
};
use super::reader::ByteReader;
use super::types::{ColumnMetadata, Portal, PreparedStatement, StateHandlingResult};
use super::{
    AuthenticationOk, CommandComplete, DataRow, Decode, Encode, ErrorResponse,
    ErrorResponseSeverity, ProtocolState, Query, ReadyForQuery, RowDescription, SQLCommand,
    WireProtocolStates,
};
use crate::cache::store::{cache_key, cache_key_wire};
use crate::{AppState, wire::messages::DescribeMessageContentTarget};
use sqlx::{
    Column, Executor, Row, Statement,
    postgres::{PgColumn, PgRow},
};
use std::time::{Duration, Instant};
use std::{collections::HashMap, io, sync::Arc};
use tokio::net::TcpStream;

// pub(super) async fn waiting_for_ssl(protocol_state: &ProtocolState) -> StateHandlingResult {
//     let response = b"N";
//     match stream_try_write(&protocol_state.stream, response).await {
//         Some(_) => StateHandlingResult::Continue(WireProtocolStates::WaitingForStartup),
//         None => StateHandlingResult::Error("failed to write SSL response".to_string()),
//     }
// }
// pub(super) async fn waiting_for_startup(protocol_state: &ProtocolState) -> StateHandlingResult {
//     let auth_ok = &AuthenticationOk.encode();
//     stream_try_write(&protocol_state.stream, &auth_ok).await;
//     let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
//     match stream_try_write(&protocol_state.stream, &ready_for_query).await {
//         Some(_) => StateHandlingResult::Continue(WireProtocolStates::ReadyForQuery),
//         None => StateHandlingResult::Error("failed to write auth ok response".to_string()),
//     }
// }
// pub(super) async fn ready_for_query(
//     buffer_length: usize,
//     protocol_state: &mut ProtocolState,
// ) -> StateHandlingResult {
//     let mut reader = ByteReader::new(&protocol_state.read_buffer[..buffer_length], 0);
//     match reader.crawl_and_find_messages() {
//         Ok(messages) => {
//             for message in messages {
//                 match message {
//                     QueryMessage(content) => match query_message(content, protocol_state).await {
//                         Ok(_) => {
//                             return StateHandlingResult::Continue(
//                                 WireProtocolStates::ReadyForQuery,
//                             );
//                         }
//                         Err(err) => return err,
//                     },
//                     ParseMessage(content) => {
//                         if let Err(err) = parse_message(content, protocol_state).await {
//                             stream_try_write(
//                                 &protocol_state.stream,
//                                 &ErrorResponse {
//                                     severity: ErrorResponseSeverity::Error,
//                                     error_message: "an internal error occured".to_string(),
//                                     sql_state_code: "XX000".to_string(),
//                                 }
//                                 .encode(),
//                             )
//                             .await;
//                             return err;
//                         }
//                     }
//                     BindMessage(content) => {
//                         if let Err(err) = bind_message(content, protocol_state).await {
//                             stream_try_write(
//                                 &protocol_state.stream,
//                                 &ErrorResponse {
//                                     severity: ErrorResponseSeverity::Error,
//                                     error_message: "an internal error occured".to_string(),
//                                     sql_state_code: "XX000".to_string(),
//                                 }
//                                 .encode(),
//                             )
//                             .await;
//                             return err;
//                         }
//                     }
//                     DescribeMessage(content) => {
//                         if let Err(err) = describe_message(content, protocol_state).await {
//                             stream_try_write(
//                                 &protocol_state.stream,
//                                 &ErrorResponse {
//                                     severity: ErrorResponseSeverity::Error,
//                                     error_message: "an internal error occured".to_string(),
//                                     sql_state_code: "XX000".to_string(),
//                                 }
//                                 .encode(),
//                             )
//                             .await;
//                             return err;
//                         }
//                     }
//                     ExecuteMessage(content) => {
//                         if let Err(err) = execute_message(content, protocol_state).await {
//                             stream_try_write(
//                                 &protocol_state.stream,
//                                 &ErrorResponse {
//                                     severity: ErrorResponseSeverity::Error,
//                                     error_message: "an internal error occured".to_string(),
//                                     sql_state_code: "XX000".to_string(),
//                                 }
//                                 .encode(),
//                             )
//                             .await;
//                             return err;
//                         }
//                     }
//                     SyncMessage => {
//                         println!(" ------ Sync message Sent  ------");
//                         let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
//                         stream_try_write(&protocol_state.stream, &ready_for_query).await;
//                         println!("sent Sync message response");
//                         return StateHandlingResult::Continue(WireProtocolStates::ReadyForQuery);
//                     }
//                     TerminateMessage => {
//                         println!(" ------ Terminate message Sent  ------");
//                         return StateHandlingResult::Break(
//                             "client sent terminate message".to_string(),
//                         );
//                     }
//                     _ => {}
//                 }
//             }
//             return StateHandlingResult::Continue(WireProtocolStates::ReadyForQuery);
//         }
//         Err(err) => {
//             stream_try_write(
//                 &protocol_state.stream,
//                 &ErrorResponse {
//                     severity: ErrorResponseSeverity::Error,
//                     error_message: err.message,
//                     sql_state_code: "XX000".to_string(),
//                 }
//                 .encode(),
//             )
//             .await;
//             let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
//             stream_try_write(&protocol_state.stream, &ready_for_query).await;
//             StateHandlingResult::Continue(WireProtocolStates::ReadyForQuery)
//         }
//     }

//     // match (Query {
//     //     bytes: protocol_state.read_buffer[5..buffer_length].to_vec(),
//     // }
//     // .decode())
//     // {
//     //     Ok(message) => {
//     //         let query_string = message.query;
//     //         match command_tag_from_query_str(&query_string) {
//     //             Some(command_tag) => {
//     //                 println!("Decoded query: {}", query_string);
//     //                 match get_from_cache(protocol_state, &query_string) {
//     //                     Some(cached_result) => {
//     //                         stream_try_write(&protocol_state.stream, &cached_result).await;
//     //                     }
//     //                     None => {
//     //                         match execute_query(&query_string, &protocol_state.app_state).await {
//     //                             Ok(results) => {
//     //                                 if let Some(bytes) = create_response_bytes(
//     //                                     results,
//     //                                     &command_tag,
//     //                                     &protocol_state.app_state,
//     //                                 ) {
//     //                                     stream_try_write(&protocol_state.stream, &bytes).await;
//     //                                     set_in_cache(protocol_state, &query_string, bytes);
//     //                                 }
//     //                             }
//     //                             Err(err) => {
//     //                                 stream_try_write(
//     //                                     &protocol_state.stream,
//     //                                     &create_error_message(ErrorResponseSeverity::Error, err),
//     //                                 )
//     //                                 .await;
//     //                             }
//     //                         }
//     //                     }
//     //                 }
//     //                 let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
//     //                 stream_try_write(&protocol_state.stream, &ready_for_query).await;
//     //             }
//     //             None => {
//     //                 stream_try_write(
//     //                     &protocol_state.stream,
//     //                     &ErrorResponse {
//     //                         severity: ErrorResponseSeverity::Error,
//     //                         error_message:
//     //                             "error while creating command tag from query, is your query malformed or incomplete?"
//     //                                 .to_string(),
//     //                         sql_state_code: "XX000".to_string(),
//     //                     }
//     //                     .encode(),
//     //                 )
//     //                 .await;
//     //                 let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
//     //                 stream_try_write(&protocol_state.stream, &ready_for_query).await;
//     //             }
//     //         };
//     //         StateHandlingResult::Continue(WireProtocolStates::ReadyForQuery)
//     //     }
//     //     Err(err) => {
//     //         stream_try_write(
//     //             &protocol_state.stream,
//     //             &ErrorResponse {
//     //                 severity: ErrorResponseSeverity::Error,
//     //                 error_message: err.message,
//     //                 sql_state_code: "XX000".to_string(),
//     //             }
//     //             .encode(),
//     //         )
//     //         .await;
//     //         let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
//     //         stream_try_write(&protocol_state.stream, &ready_for_query).await;
//     //         StateHandlingResult::Continue(WireProtocolStates::ReadyForQuery)
//     //     }
//     // }
// }

// async fn execute_query(
//     query_string: &str,
//     app_state: &AppState,
// ) -> Result<Vec<PgRow>, sqlx::Error> {
//     let query = sqlx::query(&query_string);
//     match query.fetch_all(app_state.pool.as_ref()).await {
//         Ok(response) => Ok(response),
//         Err(err) => {
//             eprintln!("Error occurred: {}", err);
//             Err(err)
//         }
//     }
// }

// async fn execute_query_prepared(
//     portal: &Portal,
//     prepared_statement: &PreparedStatement,
//     app_state: &AppState,
// ) -> Result<Vec<PgRow>, sqlx::Error> {
//     let mut query = sqlx::query(&prepared_statement.query);
//     for param in &portal.parameter_values {
//         match param {
//             None => {}
//             Some(value) => {
//                 if let Ok(value_as_string) = String::from_utf8(value.clone()) {
//                     query = query.bind(value_as_string);
//                 }
//             }
//         }
//     }

//     match query.fetch_all(app_state.pool.as_ref()).await {
//         Ok(response) => Ok(response),
//         Err(err) => {
//             eprintln!("Error occurred: {}", err);
//             Err(err)
//         }
//     }
// }

// async fn get_metadata(
//     query: &str,
//     app_state: &AppState,
// ) -> Result<Vec<ColumnMetadata>, sqlx::Error> {
//     let metadata = app_state.pool.as_ref().prepare(query).await?;
//     column_metadata_from_pg_column(metadata.columns(), app_state)
// }

// fn column_metadata_from_pg_column(
//     columns: &[PgColumn],
//     app_state: &AppState,
// ) -> Result<Vec<ColumnMetadata>, sqlx::Error> {
//     let mut column_metadata: Vec<ColumnMetadata> = Vec::new();
//     for column in columns {
//         let name = column.name().to_string();
//         let attribute_number = match column.relation_attribute_no() {
//             Some(number) => number,
//             None => 0,
//         };
//         let table_oid = match column.relation_id() {
//             Some(oid) => oid.0 as i32,
//             None => 0,
//         };
//         let type_oid = match column.type_info().oid() {
//             Some(oid) => oid.0 as i32,
//             None => 0,
//         };
//         let type_len = match app_state.pg_type_lens.get(&(type_oid as u32)) {
//             Some(&type_len) => type_len,
//             None => -2,
//         };
//         column_metadata.push(ColumnMetadata {
//             name,
//             attribute_number,
//             table_oid,
//             type_len,
//             type_oid,
//         });
//     }
//     Ok(column_metadata)
// }

// fn create_response_bytes_simple(
//     query_results: Vec<PgRow>,
//     command_tag: &SQLCommand,
//     app_state: &AppState,
// ) -> Option<Vec<u8>> {
//     let columns = match query_results.first() {
//         Some(row) => row.columns(),
//         None => return None,
//     };
//     let mut response: Vec<u8> = Vec::new();
//     let column_metadata = match column_metadata_from_pg_column(columns, app_state) {
//         Ok(metadata) => metadata,
//         Err(_) => return None,
//     };
//     response.extend(
//         RowDescription {
//             columns: &column_metadata,
//         }
//         .encode(),
//     );
//     for row in &query_results {
//         response.extend(DataRow { row: row }.encode());
//     }
//     response.extend(
//         CommandComplete {
//             rows: query_results.len() as u16,
//             command_tag: command_tag,
//         }
//         .encode(),
//     );

//     println!("----------------------\nCOMMAND COMPLETE\n----------------------");
//     Some(response)
// }

// fn create_response_bytes(query_results: Vec<PgRow>, command_tag: &SQLCommand) -> Vec<u8> {
//     let mut response: Vec<u8> = Vec::new();
//     for row in &query_results {
//         response.extend(DataRow { row: row }.encode());
//     }
//     response.extend(
//         CommandComplete {
//             rows: query_results.len() as u16,
//             command_tag: command_tag,
//         }
//         .encode(),
//     );

//     println!("----------------------\nCOMMAND COMPLETE\n----------------------");
//     response
// }

// fn command_tag_from_query_str(query: &str) -> Option<SQLCommand> {
//     let query = query.to_uppercase();
//     let split = query.split(" ").collect::<Vec<&str>>();
//     let first = split[0];
//     match first {
//         "SELECT" => Some(SQLCommand::Select),
//         "INSERT" => Some(SQLCommand::Insert),
//         "UPDATE" => Some(SQLCommand::Update),
//         "DELETE" => Some(SQLCommand::Delete),
//         "MERGE" => Some(SQLCommand::Merge),
//         "MOVE" => Some(SQLCommand::Move),
//         "FETCH" => Some(SQLCommand::Fetch),
//         "COPY" => Some(SQLCommand::Copy),
//         "CREATE" => {
//             let (second, third) = (split.get(1), split.get(2));
//             if second.unwrap_or(&&"") == &"TABLE" && third.unwrap_or(&&"") == &"AS" {
//                 Some(SQLCommand::CreateTableAs)
//             } else {
//                 None
//             }
//         }
//         _ => None,
//     }
// }

// pub(super) async fn stream_try_write(stream: &TcpStream, buf: &[u8]) -> Option<usize> {
//     let mut written = 0;
//     while written < buf.len() {
//         stream.writable().await.ok()?;
//         match stream.try_write(&buf[written..]) {
//             Ok(n) => written += n,
//             Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => continue,
//             Err(err) => {
//                 eprintln!("Error occurred: {}", err);
//                 return None;
//             }
//         }
//     }
//     Some(written)
// }

// fn create_error_message(severity: ErrorResponseSeverity, err: sqlx::Error) -> Vec<u8> {
//     let error_message: String;
//     let sql_state_code: String;
//     match err.as_database_error() {
//         Some(err) => {
//             error_message = err.message().to_string();
//             match err.code() {
//                 Some(code) => sql_state_code = code.to_string(),
//                 None => sql_state_code = "XX000".to_string(), // Pledge undefined error
//             }
//         }
//         None => {
//             error_message = "Unknown error".to_string();
//             sql_state_code = "XX000".to_string(); // Pledge undefined error
//         }
//     }

//     let error = ErrorResponse {
//         severity: severity,
//         error_message: error_message,
//         sql_state_code: sql_state_code,
//     }
//     .encode();

//     error
// }

// fn get_from_cache_simple(protocol_state: &ProtocolState, query: &str) -> Option<Vec<u8>> {
//     if protocol_state.app_state.matcher.template_exists(query) {
//         let key = cache_key(query, &Vec::new());
//         return protocol_state.app_state.cache.get(&key);
//     }
//     None
// }

// fn get_from_cache(protocol_state: &ProtocolState, query: &str, key: &str) -> Option<Vec<u8>> {
//     if protocol_state.app_state.matcher.template_exists(query) {
//         return protocol_state.app_state.cache.get(&key);
//     }
//     None
// }

// fn set_in_cache(protocol_state: &ProtocolState, query: &str, key: &str, result: Vec<u8>) {
//     println!("cache_key set: {}", query);
//     if let Some(template) = protocol_state.app_state.matcher.find_template(query) {
//         let expiration = match template.ttl {
//             Some(ttl) => Instant::now() + Duration::from_secs(ttl),
//             None => Instant::now() + Duration::from_secs(protocol_state.app_state.global_ttl),
//         };
//         protocol_state
//             .app_state
//             .cache
//             .insert(key.to_string(), result, expiration);
//     }
// }
// async fn query_message(
//     content: QueryMessageContent,
//     protocol_state: &mut ProtocolState,
// ) -> Result<(), StateHandlingResult> {
//     match command_tag_from_query_str(&content.query) {
//         Some(command_tag) => {
//             println!("Decoded query: {}", content.query);
//             match get_from_cache_simple(protocol_state, &content.query) {
//                 Some(cached_result) => {
//                     stream_try_write(&protocol_state.stream, &cached_result).await;
//                 }
//                 None => match execute_query(&content.query, &protocol_state.app_state).await {
//                     Ok(results) => {
//                         if let Some(bytes) = create_response_bytes_simple(
//                             results,
//                             &command_tag,
//                             &protocol_state.app_state,
//                         ) {
//                             stream_try_write(&protocol_state.stream, &bytes).await;
//                             set_in_cache(protocol_state, &content.query, &content.query, bytes);
//                         }
//                     }
//                     Err(err) => {
//                         stream_try_write(
//                             &protocol_state.stream,
//                             &create_error_message(ErrorResponseSeverity::Error, err),
//                         )
//                         .await;
//                     }
//                 },
//             }
//             let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
//             stream_try_write(&protocol_state.stream, &ready_for_query).await;
//         }
//         None => {
//             stream_try_write(
//                         &protocol_state.stream,
//                         &ErrorResponse {
//                             severity: ErrorResponseSeverity::Error,
//                             error_message:
//                                 "error while creating command tag from query, is your query malformed or incomplete?"
//                                     .to_string(),
//                             sql_state_code: "XX000".to_string(),
//                         }
//                         .encode(),
//                     )
//                     .await;
//             let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
//             stream_try_write(&protocol_state.stream, &ready_for_query).await;
//         }
//     };
//     Ok(())
// }

// async fn parse_message(
//     content: ParseMessageContent,
//     protocol_state: &mut ProtocolState,
// ) -> Result<(), StateHandlingResult> {
//     let column_metadata = match get_metadata(&content.query, &protocol_state.app_state).await {
//         Ok(metadata) => metadata,
//         Err(err) => {
//             return Err(StateHandlingResult::Error(err.to_string()));
//         }
//     };
//     let prepared_statement = PreparedStatement {
//         query: content.query,
//         parameter_data_types: content.parameter_data_types,
//         column_metadata,
//     };
//     protocol_state
//         .prepared_statements
//         .insert(content.prepared_statement_name, prepared_statement);

//     let parse_complete = &ParseComplete.encode();
//     stream_try_write(&protocol_state.stream, &parse_complete).await;
//     println!("sent Parse message response");
//     Ok(())
// }

// async fn bind_message(
//     content: BindMessageContent,
//     protocol_state: &mut ProtocolState,
// ) -> Result<(), StateHandlingResult> {
//     let portal = Portal {
//         source_prepared_statement_name: content.source_prepared_statement_name,
//         parameter_format_codes: content.parameter_format_codes,
//         parameter_values: content.parameter_values,
//         result_column_format_codes: content.result_column_format_codes,
//     };
//     protocol_state.portals.insert(content.portal_name, portal);
//     let bind_complete = &BindComplete.encode();
//     stream_try_write(&protocol_state.stream, &bind_complete).await;
//     println!("sent Bind message response");
//     Ok(())
// }

// async fn describe_message(
//     content: DescribeMessageContent,
//     protocol_state: &mut ProtocolState,
// ) -> Result<(), StateHandlingResult> {
//     let portal = match protocol_state.portals.get(&content.name) {
//         Some(portal) => portal,
//         None => {
//             let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
//             stream_try_write(&protocol_state.stream, &ready_for_query).await;
//             return Err(StateHandlingResult::Continue(
//                 WireProtocolStates::ReadyForQuery,
//             ));
//         }
//     };
//     let metadata = match protocol_state
//         .prepared_statements
//         .get(&portal.source_prepared_statement_name)
//     {
//         Some(statement) => &statement.column_metadata,
//         None => {
//             let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
//             stream_try_write(&protocol_state.stream, &ready_for_query).await;
//             return Err(StateHandlingResult::Continue(
//                 WireProtocolStates::ReadyForQuery,
//             ));
//         }
//     };
//     let row_description = &RowDescription { columns: metadata }.encode();
//     match content.target {
//         DescribeMessageContentTarget::Portal => {
//             stream_try_write(&protocol_state.stream, &row_description).await;
//         }
//         DescribeMessageContentTarget::PreparedStatement => {
//             // TODO
//             // Send ParameterDescription before RowDescription
//         }
//     }
//     println!("sent Describe message response");
//     Ok(())
// }

// async fn execute_message(
//     content: ExecuteMessageContent,
//     protocol_state: &mut ProtocolState,
// ) -> Result<(), StateHandlingResult> {
//     let portal = match protocol_state.portals.get(&content.name) {
//         Some(portal) => portal,
//         None => {
//             return Err(StateHandlingResult::Error(
//                 "invalid portal name".to_string(),
//             ));
//         }
//     };
//     let prepared_statement = match protocol_state
//         .prepared_statements
//         .get(&portal.source_prepared_statement_name)
//     {
//         Some(prepared_statement) => prepared_statement,
//         None => {
//             return Err(StateHandlingResult::Error(
//                 "invalid source prepared statement name".to_string(),
//             ));
//         }
//     };
//     let command_tag = match command_tag_from_query_str(&prepared_statement.query) {
//         Some(tag) => tag,
//         None => {
//             stream_try_write(
//                 &protocol_state.stream,
//                 &ErrorResponse {
//                     severity: ErrorResponseSeverity::Error,
//                     error_message:
//                         "error while creating command tag from query, is your query malformed or incomplete?"
//                             .to_string(),
//                     sql_state_code: "XX000".to_string(),
//                 }
//                 .encode(),
//             )
//             .await;
//             let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
//             stream_try_write(&protocol_state.stream, &ready_for_query).await;
//             return Err(StateHandlingResult::Continue(
//                 WireProtocolStates::ReadyForQuery,
//             ));
//         }
//     };
//     let cache_key = cache_key_wire(&prepared_statement.query, &portal.parameter_values);
//     match get_from_cache(protocol_state, &prepared_statement.query, &cache_key) {
//         Some(cached_result) => {
//             stream_try_write(&protocol_state.stream, &cached_result).await;
//         }
//         None => {
//             match execute_query_prepared(&portal, &prepared_statement, &protocol_state.app_state)
//                 .await
//             {
//                 Ok(results) => {
//                     let bytes = create_response_bytes(results, &command_tag);
//                     stream_try_write(&protocol_state.stream, &bytes).await;
//                     set_in_cache(protocol_state, &prepared_statement.query, &cache_key, bytes);
//                 }
//                 Err(err) => {
//                     stream_try_write(
//                         &protocol_state.stream,
//                         &create_error_message(ErrorResponseSeverity::Error, err),
//                     )
//                     .await;
//                 }
//             }
//         }
//     }

//     // println!(
//     //     " ------ Execute message START:  ------ \nname: '{}', rows_to_return_limit: '{}'\n -----  Execute message END ----- \n",
//     //     content.name, content.rows_to_return_limit,
//     // );
//     println!("sent Execute message response");
//     Ok(())
// }
