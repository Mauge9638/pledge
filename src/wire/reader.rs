use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::wire::Decode;

use super::messages::{Parse, Query, QueryMessageContent};

use super::messages::ClientMessageContent;

pub(super) enum ByteReaderErrorKind {
    EndOfBuffer,
    ErrorParsing,
    OutOfBounds,
}

pub(super) struct ByteReaderError {
    pub kind: ByteReaderErrorKind,
    pub message: String,
}

pub(super) struct ByteReader<'a> {
    buffer: &'a [u8],
    cursor: usize,
}

impl<'a> ByteReader<'a> {
    pub(super) fn new(buffer: &'a [u8], cursor: usize) -> Self {
        Self { buffer, cursor }
    }
    pub(super) fn update_cursor(&mut self, new_cursor: usize) {
        self.cursor = new_cursor;
    }

    pub(super) fn crawl_and_find_messages(
        &mut self,
    ) -> Result<Vec<ClientMessageContent>, ByteReaderError> {
        let mut messages: Vec<ClientMessageContent> = Vec::new();
        loop {
            if self.cursor >= self.buffer.len() {
                break;
            }
            let type_byte = &self.buffer[self.cursor];
            self.cursor += 1;
            let message_length = self.read_i32()?;
            match type_byte {
                b'Q' => {
                    let decode = Query {
                        bytes: self.buffer
                            [self.cursor..(self.cursor + (message_length - 4) as usize)]
                            .to_vec(),
                    }
                    .decode();
                    messages.push(match decode {
                        Ok(content) => ClientMessageContent::QueryMessage(content),
                        Err(err) => return Err(err),
                    })
                }
                b'P' => {
                    let decode = Parse {
                        bytes: self.buffer
                            [self.cursor..(self.cursor + (message_length - 4) as usize)]
                            .to_vec(),
                    }
                    .decode();
                    messages.push(match decode {
                        Ok(content) => ClientMessageContent::ParseMessage(content),
                        Err(err) => return Err(err),
                    })
                } // Parse
                b'B' => println!("identification byte: 'Bind'"),
                b'E' => println!("identification byte: 'Execute'"), // Execute
                b'S' => println!("identification byte: 'Sync'"),    // Sync
                b'D' => println!("identification byte: 'Describe'"), // Describe
                b'C' => println!("identification byte: 'Close'"),   // Close
                b'X' => println!("identification byte: 'Terminate'"), // Terminate
                _ => println!("type_byte byte: 'Unknown byte {}'", type_byte),
            }
            self.cursor += (message_length - 4) as usize;
        }

        Ok(messages)
    }

    pub(super) fn read_cstring(&mut self) -> Result<String, ByteReaderError> {
        let start_cursor = self.cursor;
        loop {
            if self.cursor >= self.buffer.len() {
                return Err(self.get_end_of_buffer_error());
            }
            // 0u8 is the normal cstring null terminator (basically just a byte with the value 0)
            else if self.buffer[self.cursor] == 0u8 {
                // println!("cursor is at: {} and hit a null terminator", self.cursor);
                self.cursor += 1;
                break;
            }
            self.cursor += 1;
        }

        // println!("Cursor is at:{} and is trying to parse now", self.cursor);
        match String::from_utf8(self.buffer[start_cursor..self.cursor - 1].to_vec()) {
            Ok(string) => Ok(string),
            Err(err) => Err(ByteReaderError {
                kind: ByteReaderErrorKind::ErrorParsing,
                message: format!("{}", err),
            }),
        }
    }
    pub(super) fn read_i16(&mut self) -> Result<i16, ByteReaderError> {
        if (self.cursor) >= self.buffer.len() {
            return Err(self.get_end_of_buffer_error());
        }
        if (self.cursor + 2) >= self.buffer.len() {
            return Err(self.get_out_of_bounds_error(2));
        }
        let mut bytes = BytesMut::with_capacity(0);
        bytes.extend_from_slice(&self.buffer[self.cursor..(self.cursor + 2)]);
        let value: i16 = bytes.get_i16();
        self.cursor += 2;
        Ok(value)
    }
    pub(super) fn read_i32(&mut self) -> Result<i32, ByteReaderError> {
        if (self.cursor) >= self.buffer.len() {
            return Err(self.get_end_of_buffer_error());
        }
        if (self.cursor + 4) >= self.buffer.len() {
            return Err(self.get_out_of_bounds_error(4));
        }
        let value: [u8; 4] = match self.buffer[self.cursor..(self.cursor + 4)].try_into() {
            Ok(data) => data,
            Err(_) => return Err(self.get_end_of_buffer_error()),
        };

        let value: i32 = i32::from_be_bytes(value);
        self.cursor += 4;
        Ok(value)
    }
    // pub(super) fn read_i32(&mut self) -> Result<i32, ByteReaderError> {
    //     if (self.cursor) >= self.buffer.len() {
    //         return Err(self.get_end_of_buffer_error());
    //     }
    //     if (self.cursor + 4) >= self.buffer.len() {
    //         return Err(self.get_out_of_bounds_error(4));
    //     }

    //     let mut bytes = BytesMut::with_capacity(0);
    //     bytes.extend_from_slice(&self.buffer[self.cursor..(self.cursor + 4)]);
    //     let value: i32 = bytes.get_i32();
    //     self.cursor += 4;
    //     Ok(value)
    // }

    pub(super) fn read_bytes(&mut self, bytes_len: usize) -> Result<Vec<u8>, ByteReaderError> {
        if (self.cursor) >= self.buffer.len() {
            return Err(self.get_end_of_buffer_error());
        }
        if (self.cursor + bytes_len) >= self.buffer.len() {
            return Err(self.get_out_of_bounds_error(4));
        }

        let mut bytes = BytesMut::with_capacity(0);
        bytes.extend_from_slice(&self.buffer[self.cursor..(self.cursor + bytes_len)]);

        let value: u8 = bytes.get_u8()
        self.cursor += bytes_len;
        Ok(value)
    }

    fn get_end_of_buffer_error(&self) -> ByteReaderError {
        ByteReaderError {
            kind: ByteReaderErrorKind::EndOfBuffer,
            message: "cursor is at end of buffer, no more data to read".to_string(),
        }
    }

    fn get_out_of_bounds_error(&self, extra_bytes: usize) -> ByteReaderError {
        ByteReaderError {
            kind: ByteReaderErrorKind::EndOfBuffer,
            message: format!(
                "taking the required {} bytes exceeds buffer length",
                extra_bytes
            ),
        }
    }
}
