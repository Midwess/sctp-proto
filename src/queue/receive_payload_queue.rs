use crate::chunk::chunk_payload_data::ChunkPayloadData;
use crate::chunk::chunk_selective_ack::GapAckBlock;
use crate::util::{sna32gt, sna32lte};

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::{max, min};

const AVG_CHUNK_SIZE: u32 = 500;
const MIN_TSN_OFFSET: u32 = 2_000;
const MAX_TSN_OFFSET: u32 = 40_000;

#[derive(Debug, Clone)]
pub(crate) struct ReceivePayloadQueue {
    tail_tsn: u32,
    chunk_size: usize,
    tsn_bitmask: Vec<u64>,
    dup_tsn: Vec<u32>,
    max_tsn_offset: u32,
    cumulative_tsn: u32,
}

impl Default for ReceivePayloadQueue {
    fn default() -> Self {
        Self::new(MAX_TSN_OFFSET)
    }
}

impl ReceivePayloadQueue {
    pub(crate) fn new(max_tsn_offset: u32) -> Self {
        let max_tsn_offset = max(64, ((max_tsn_offset + 63) / 64) * 64);
        Self {
            tail_tsn: 0,
            chunk_size: 0,
            tsn_bitmask: vec![0; (max_tsn_offset / 64) as usize],
            dup_tsn: vec![],
            max_tsn_offset,
            cumulative_tsn: 0,
        }
    }

    pub(crate) fn from_max_receive_buffer_size(max_receive_buffer_size: u32) -> Self {
        let offset = min(
            max((max_receive_buffer_size.saturating_mul(4)) / AVG_CHUNK_SIZE, MIN_TSN_OFFSET),
            MAX_TSN_OFFSET,
        );
        Self::new(offset)
    }

    pub(crate) fn init(&mut self, cumulative_tsn: u32) {
        self.cumulative_tsn = cumulative_tsn;
        self.tail_tsn = cumulative_tsn;
        self.chunk_size = 0;
        for mask in &mut self.tsn_bitmask {
            *mask = 0;
        }
        self.dup_tsn.clear();
    }

    pub(crate) fn cumulative_tsn(&self) -> u32 {
        self.cumulative_tsn
    }

    fn has_chunk(&self, tsn: u32) -> bool {
        if self.chunk_size == 0 || sna32lte(tsn, self.cumulative_tsn) || sna32gt(tsn, self.tail_tsn)
        {
            return false;
        }

        let index = (tsn / 64) as usize % self.tsn_bitmask.len();
        let offset = tsn % 64;
        (self.tsn_bitmask[index] & (1 << offset)) != 0
    }

    pub(crate) fn can_push(&self, p: &ChunkPayloadData) -> bool {
        let tsn = p.tsn;
        !self.has_chunk(tsn)
            && !sna32lte(tsn, self.cumulative_tsn)
            && !sna32gt(tsn, self.cumulative_tsn + self.max_tsn_offset)
    }

    pub(crate) fn push(&mut self, p: ChunkPayloadData) -> bool {
        let tsn = p.tsn;
        if sna32gt(tsn, self.cumulative_tsn + self.max_tsn_offset) {
            return false;
        }

        if sna32lte(tsn, self.cumulative_tsn) || self.has_chunk(tsn) {
            self.dup_tsn.push(tsn);
            return false;
        }

        let index = (tsn / 64) as usize % self.tsn_bitmask.len();
        let offset = tsn % 64;
        self.tsn_bitmask[index] |= 1 << offset;
        self.chunk_size += 1;
        if sna32gt(tsn, self.tail_tsn) {
            self.tail_tsn = tsn;
        }

        true
    }

    pub(crate) fn pop(&mut self, force: bool) -> bool {
        let tsn = self.cumulative_tsn.wrapping_add(1);
        if self.has_chunk(tsn) {
            let index = (tsn / 64) as usize % self.tsn_bitmask.len();
            let offset = tsn % 64;
            self.tsn_bitmask[index] &= !(1 << offset);
            self.chunk_size -= 1;
            self.cumulative_tsn = self.cumulative_tsn.wrapping_add(1);
            return true;
        }

        if force {
            self.cumulative_tsn = self.cumulative_tsn.wrapping_add(1);
            if self.chunk_size == 0 {
                self.tail_tsn = self.cumulative_tsn;
            }
        }

        false
    }

    pub(crate) fn pop_duplicates(&mut self) -> Vec<u32> {
        self.dup_tsn.drain(..).collect()
    }

    pub(crate) fn get_gap_ack_blocks(&self) -> Vec<GapAckBlock> {
        if self.chunk_size == 0 {
            return vec![];
        }

        let mut gap_ack_blocks = vec![];
        let mut tsn = self.cumulative_tsn.wrapping_add(1);
        let end_tsn = self.tail_tsn;
        let mut current: Option<GapAckBlock> = None;

        while sna32lte(tsn, end_tsn) {
            if self.has_chunk(tsn) {
                let diff = tsn.wrapping_sub(self.cumulative_tsn) as u16;
                match &mut current {
                    Some(block) if block.end + 1 == diff => block.end = diff,
                    Some(block) => {
                        gap_ack_blocks.push(*block);
                        *block = GapAckBlock {
                            start: diff,
                            end: diff,
                        };
                    }
                    None => {
                        current = Some(GapAckBlock {
                            start: diff,
                            end: diff,
                        });
                    }
                }
            }
            tsn = tsn.wrapping_add(1);
        }

        if let Some(block) = current {
            gap_ack_blocks.push(block);
        }

        gap_ack_blocks
    }

    pub(crate) fn get_gap_ack_blocks_string(&self) -> String {
        let mut s = format!("cumTSN={}", self.cumulative_tsn);
        for block in self.get_gap_ack_blocks() {
            s += format!(",{}-{}", block.start, block.end).as_str();
        }
        s
    }

    pub(crate) fn get_last_tsn_received(&self) -> Option<&u32> {
        if self.chunk_size == 0 {
            None
        } else {
            Some(&self.tail_tsn)
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.chunk_size
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.chunk_size == 0
    }
}
