use std::{
    collections::BTreeMap,
    ops::Range,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    AppState,
    cache::{QueryTemplate, lfu::CachedResponse, store::cache_key_wire},
    wire::types::{CacheCommandCapture, CacheCommandReplay, CachePlan},
};

use super::{
    messages::{
        BindMessageContent, ClientMessageContent,
        ClientMessageContent::{
            BindMessage, DescribeMessage, ExecuteMessage, ParseMessage, QueryMessage, SyncMessage,
            UnknownMessage,
        },
        DescribeMessageContent, DescribeMessageContentTarget, ExecuteMessageContent,
        ParseMessageContent,
    },
    types::{
        CacheCommand, ClientState, DescribeKind, Portal, PreparedStatement, ProtocolMode,
        ReplayTrim, ReplayTrimExtended, ReplayTrimSimple, StateHandlingResult,
    },
};

pub(super) fn get_from_cache(client_state: &ClientState, key: &str) -> Option<Arc<CachedResponse>> {
    return client_state.app_state.cache.get(&key);
}

pub(super) fn set_in_cache(app_state: &AppState, ttl: Duration, key: &str, data: CachedResponse) {
    println!("cache_key set: {}", key);
    app_state
        .cache
        .insert(key.to_string(), data, Instant::now() + ttl);
}

pub(super) fn find_template(
    content: &ExecuteMessageContent,
    client_state: &mut ClientState,
) -> Option<CachePlan> {
    let portal = match client_state.portals.get(&content.name) {
        Some(portal) => portal,
        None => return None,
    };
    let prepared_statement = match client_state
        .prepared_statements
        .get(&portal.source_prepared_statement_name)
    {
        Some(prepared_statement) => prepared_statement,
        None => return None,
    };

    if let Some(ttl) = resolve_ttl(&client_state.app_state, &prepared_statement.query) {
        return Some(CachePlan {
            key: cache_key_wire(&prepared_statement.query, &portal.parameter_values),
            ttl: ttl,
        });
    }
    None
}

pub(super) fn find_template_simple(
    query: &String,
    client_state: &mut ClientState,
) -> Option<CachePlan> {
    if let Some(ttl) = resolve_ttl(&client_state.app_state, &query) {
        return Some(CachePlan {
            key: cache_key_wire(query, &Vec::new()),
            ttl: ttl,
        });
    }
    None
}

pub(super) fn resolve_ttl(app_state: &AppState, query: &str) -> Option<Duration> {
    let ttl = match app_state.matcher.find_template(&query) {
        Some(template) => match template.ttl {
            Some(ttl) => Duration::from_secs(ttl),
            None => Duration::from_secs(app_state.global_ttl),
        },
        None => return None,
    };
    return Some(ttl);
}

pub(super) async fn find_cache_related_messages(
    messages: Vec<ClientMessageContent>,
    client_state: &mut ClientState,
) -> (BTreeMap<u16, CacheCommand>, Vec<ReplayTrim>, bool) {
    let mut should_hit_db = false; // This just signals if a non cache configured query is in the messages
    let mut cache_commands: BTreeMap<u16, CacheCommand> = BTreeMap::new();
    let mut replay_trims: Vec<ReplayTrim> = Vec::new();
    let mut parse_messages: Vec<(ParseMessageContent, usize, usize)> = Vec::new();
    let mut bind_messages: Vec<(BindMessageContent, usize, usize)> = Vec::new();
    let mut describe_messages: Vec<(DescribeMessageContent, usize, usize)> = Vec::new();
    let mut sync_range: Range<usize> = 0..0;
    let mut order = 0;
    for message in messages {
        match message {
            QueryMessage { data, start, end } => 'query: {
                let cache_plan = match find_template_simple(&data.query, client_state) {
                    Some(cache_plan) => cache_plan,
                    None => {
                        should_hit_db = true;
                        order += 1;
                        break 'query;
                    }
                };
                let cache_command = match get_from_cache(client_state, &cache_plan.key) {
                    Some(cached_response) => CacheCommand::Replay(CacheCommandReplay {
                        data: cached_response,
                        describe_kind: DescribeKind::None,
                        protocol_mode: ProtocolMode::Simple,
                        query: data.query,
                        key: cache_plan.key,
                    }),
                    None => CacheCommand::Capture(CacheCommandCapture {
                        key: cache_plan.key,
                        describe_kind: DescribeKind::None,
                        protocol_mode: ProtocolMode::Simple,
                        query: data.query,
                        ttl: cache_plan.ttl,
                    }),
                };
                cache_commands.insert(order, cache_command);
                replay_trims.push(ReplayTrim::Simple(ReplayTrimSimple { query: start..end }));
                order += 1;
            }
            ParseMessage { data, start, end } => {
                let _ = parse_message(&data, client_state).await;
                parse_messages.push((data, start, end));
            }
            BindMessage { data, start, end } => {
                let _ = bind_message(&data, client_state).await;
                bind_messages.push((data, start, end));
            }
            DescribeMessage { data, start, end } => {
                let _ = describe_message(&data);
                describe_messages.push((data, start, end));
            }
            SyncMessage { start, end } => {
                println!("sync (start, end): ({}, {})", start, end);
                if !replay_trims.is_empty() {
                    let length = replay_trims.len();
                    let last_replay = replay_trims.get_mut(length - 1);
                    if let Some(ReplayTrim::Extended(data)) = last_replay {
                        data.sync = Some(start..end);
                    }
                }
            }
            ExecuteMessage { data, start, end } => 'execute: {
                if data.rows_to_return_limit == 0 {
                    let cache_plan = match find_template(&data, client_state) {
                        Some(cache_plan) => cache_plan,
                        None => {
                            should_hit_db = true;
                            order += 1;
                            break 'execute;
                        }
                    };
                    let cache_response = get_from_cache(client_state, &cache_plan.key);
                    let describe_kind: DescribeKind;
                    let mut paired_parse_message = None;
                    let mut paired_parse_message_query = String::new();
                    let mut paired_bind_message = None;
                    let mut paired_describe_message = None;

                    'find_describe: for (index, current_describe_message) in
                        describe_messages.clone().iter().enumerate()
                    {
                        match current_describe_message.0.target {
                            DescribeMessageContentTarget::Portal => {
                                if get_portal_in_session(
                                    &current_describe_message.0.name,
                                    client_state,
                                )
                                .is_some()
                                {
                                    paired_describe_message =
                                        Some(current_describe_message.clone());
                                    describe_messages.remove(index);
                                    break 'find_describe;
                                }
                            }
                            DescribeMessageContentTarget::PreparedStatement => {
                                if get_prepared_statement_in_session(
                                    &current_describe_message.0.name,
                                    client_state,
                                )
                                .is_some()
                                {
                                    paired_describe_message =
                                        Some(current_describe_message.clone());
                                    describe_messages.remove(index);
                                    break 'find_describe;
                                }
                            }
                        }
                    }
                    describe_kind = match &paired_describe_message {
                        Some(data) => describe_message(&data.0),
                        None => DescribeKind::None,
                    };

                    if cache_response.is_some() {
                        let portal_name = &data.name;
                        'find_bind: for (index, current_bind_message) in
                            bind_messages.clone().iter().enumerate()
                        {
                            if &current_bind_message.0.portal_name == portal_name {
                                paired_bind_message = Some(current_bind_message.clone());
                                bind_messages.remove(index);
                                'find_parse: for (index, current_parse_message) in
                                    parse_messages.clone().iter().enumerate()
                                {
                                    if current_parse_message.0.prepared_statement_name
                                        == current_bind_message.0.source_prepared_statement_name
                                    {
                                        paired_parse_message = Some(current_parse_message.clone());
                                        paired_parse_message_query =
                                            current_parse_message.0.query.clone();
                                        parse_messages.remove(index);
                                        break 'find_parse;
                                    }
                                }

                                break 'find_bind;
                            }
                        }
                    }

                    let cache_command = match &cache_response {
                        Some(cached_response)
                            if cache_data_can_satisfy(cached_response, &describe_kind) =>
                        {
                            CacheCommand::Replay(CacheCommandReplay {
                                data: cached_response.clone(),
                                describe_kind,
                                protocol_mode: ProtocolMode::Extended,
                                query: paired_parse_message_query.clone(),
                                key: cache_plan.key.clone(),
                            })
                        }
                        _ => CacheCommand::Capture(CacheCommandCapture {
                            key: cache_plan.key,
                            describe_kind,
                            protocol_mode: ProtocolMode::Extended,
                            query: paired_parse_message_query.clone(),
                            ttl: cache_plan.ttl,
                        }),
                    };

                    cache_commands.insert(order, cache_command.clone());
                    order += 1;
                    if let CacheCommand::Replay { .. } = cache_command {
                        replay_trims.push(ReplayTrim::Extended(ReplayTrimExtended {
                            execute: Range { start, end },
                            parse: {
                                if let Some(parse) = paired_parse_message {
                                    Some(parse.1..parse.2)
                                } else {
                                    None
                                }
                            },
                            bind: {
                                if let Some(bind) = paired_bind_message {
                                    Some(bind.1..bind.2)
                                } else {
                                    None
                                }
                            },
                            describe: {
                                if let Some(describe) = paired_describe_message {
                                    Some(describe.1..describe.2)
                                } else {
                                    None
                                }
                            },
                            sync: None,
                        }));
                    }
                }
            }
            UnknownMessage => should_hit_db = true,
            _ => {}
        }
    }
    if cache_commands.len() < order as usize {
        should_hit_db = true;
    }
    return (cache_commands, replay_trims, should_hit_db);
}

async fn parse_message(
    content: &ParseMessageContent,
    client_state: &mut ClientState,
) -> Result<(), StateHandlingResult> {
    if client_state
        .app_state
        .matcher
        .template_exists(&content.query)
    {
        let prepared_statement = PreparedStatement {
            query: content.query.clone(),
            parameter_data_types: content.parameter_data_types.clone(),
        };
        client_state
            .prepared_statements
            .insert(content.prepared_statement_name.clone(), prepared_statement);

        println!("saved in prepared statement");
    }
    Ok(())
}

async fn bind_message(
    content: &BindMessageContent,
    client_state: &mut ClientState,
) -> Result<(), StateHandlingResult> {
    if let Some(prepared_statement) = client_state
        .prepared_statements
        .get(&content.source_prepared_statement_name)
    {
        if client_state
            .app_state
            .matcher
            .template_exists(&prepared_statement.query)
        {
            let portal = Portal {
                source_prepared_statement_name: content.source_prepared_statement_name.clone(),
                parameter_format_codes: content.parameter_format_codes.clone(),
                parameter_values: content.parameter_values.clone(),
                result_column_format_codes: content.result_column_format_codes.clone(),
            };
            client_state
                .portals
                .insert(content.portal_name.clone(), portal);
        }
        println!("saved in portals");
    };
    Ok(())
}

fn describe_message(content: &DescribeMessageContent) -> DescribeKind {
    match content.target {
        DescribeMessageContentTarget::PreparedStatement => DescribeKind::Statement,
        DescribeMessageContentTarget::Portal => DescribeKind::Portal,
    }
}

fn get_prepared_statement_in_session<'a>(
    name: &str,
    client_state: &'a mut ClientState,
) -> Option<&'a PreparedStatement> {
    client_state.prepared_statements.get(name)
}

fn get_portal_in_session<'a>(name: &str, client_state: &'a mut ClientState) -> Option<&'a Portal> {
    client_state.portals.get(name)
}

fn cache_data_can_satisfy(response: &CachedResponse, kind: &DescribeKind) -> bool {
    match kind {
        DescribeKind::None => response.has_data(),
        DescribeKind::Portal => response.has_data() && response.has_row_desc(),
        DescribeKind::Statement => {
            response.has_data() && response.has_row_desc() && response.has_param_desc()
        }
    }
}
