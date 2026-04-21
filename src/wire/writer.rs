use super::types::SectionType;

pub(super) struct ByteWriter<'a> {
    buffer: &'a [u8],
    cursor: usize,
}

type SectionEdges = (usize, usize);

impl<'a> ByteWriter<'a> {
    pub(super) fn new(buffer: &'a [u8], cursor: usize) -> Self {
        Self { buffer, cursor }
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
}
