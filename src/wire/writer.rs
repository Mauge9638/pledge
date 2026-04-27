use std::ops::Deref;

use crate::{cache::lfu::CachedResponse, wire::CacheCommand};

use super::{ClientMessageContent, PendingCommand};

pub(super) struct ByteWriter {
    buffer: Vec<u8>,
    cursor: usize,
}

type SectionEdges = (usize, usize);

impl ByteWriter {
    pub(super) fn new(buffer: Vec<u8>, cursor: usize) -> Self {
        Self { buffer, cursor }
    }
    pub(super) fn get_buffer(&self) -> &Vec<u8> {
        &self.buffer
    }

    pub(super) fn slice_out_section(&self, start: usize, end: usize) -> Vec<u8> {
        let len = end - start;
        let mut result = Vec::with_capacity(len);
        for i in 0..len {
            result.push(self.buffer[start + i]);
        }
        result
    }
    pub(super) fn get_section_edges_parse_message(&self) -> SectionEdges {
        (0, 0)
    }
    pub(super) fn get_section_edges_query_message(&self) -> SectionEdges {
        (0, 0)
    }
    pub(super) fn trim_from_pending_commands(
        &mut self,
        pending_commands: Vec<PendingCommand>,
        parsed_messages: Vec<ClientMessageContent>,
    ) {
        let mut removed_indexes = 0;
        for command in pending_commands {
            match command {
                PendingCommand::Extended(command) => {
                    if let CacheCommand::Replay { data, .. } = command.action {
                        if let Some(parse) = command.parse {
                            for current in
                                (parse.start - removed_indexes)..(parse.end - removed_indexes)
                            {
                                self.buffer.remove(current);
                            }
                            removed_indexes += (parse.end - parse.start) + 1;
                        }
                        if let Some(bind) = command.bind {
                            for current in
                                (bind.start - removed_indexes)..(bind.end - removed_indexes)
                            {
                                self.buffer.remove(current);
                            }
                            removed_indexes += (bind.end - bind.start) + 1;
                        }
                        if let Some(describe) = command.describe {
                            for current in
                                (describe.start - removed_indexes)..(describe.end - removed_indexes)
                            {
                                self.buffer.remove(current);
                            }
                            removed_indexes += (describe.end - describe.start) + 1;
                        }
                        for current in (command.execute.start - removed_indexes)
                            ..(command.execute.end - removed_indexes)
                        {
                            self.buffer.remove(current);
                        }
                        removed_indexes += (command.execute.end - command.execute.start) + 1;
                    }
                }
                PendingCommand::Simple(command) => {
                    if let CacheCommand::Replay { .. } = command.action {
                        for current in (command.query.start - removed_indexes)
                            ..(command.query.end - removed_indexes)
                        {
                            self.buffer.remove(current);
                        }
                        removed_indexes += (command.query.end - command.query.start) + 1;
                    }
                }
            }
        }
    }
}
