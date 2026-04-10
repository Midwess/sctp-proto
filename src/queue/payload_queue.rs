use crate::chunk::chunk_payload_data::ChunkPayloadData;

use alloc::collections::VecDeque;

#[derive(Default, Debug)]
pub(crate) struct PayloadQueue {
    chunks: VecDeque<ChunkPayloadData>,
    n_bytes: usize,
}

impl PayloadQueue {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push_no_check(&mut self, p: ChunkPayloadData) {
        self.n_bytes += p.user_data.len();
        self.chunks.push_back(p);
    }

    pub(crate) fn pop(&mut self, tsn: u32) -> Option<ChunkPayloadData> {
        if self.chunks.front().is_some_and(|chunk| chunk.tsn == tsn) {
            let chunk = self.chunks.pop_front()?;
            self.n_bytes -= chunk.user_data.len();
            Some(chunk)
        } else {
            None
        }
    }

    pub(crate) fn get(&self, tsn: u32) -> Option<&ChunkPayloadData> {
        let front = self.chunks.front()?;
        if tsn < front.tsn {
            return None;
        }

        self.chunks.get((tsn - front.tsn) as usize)
    }

    pub(crate) fn get_mut(&mut self, tsn: u32) -> Option<&mut ChunkPayloadData> {
        let front = self.chunks.front()?;
        if tsn < front.tsn {
            return None;
        }

        self.chunks.get_mut((tsn - front.tsn) as usize)
    }

    pub(crate) fn mark_as_acked(&mut self, tsn: u32) -> usize {
        let Some(front_tsn) = self.chunks.front().map(|chunk| chunk.tsn) else {
            return 0;
        };
        if tsn < front_tsn {
            return 0;
        }

        let index = (tsn - front_tsn) as usize;
        let Some(c) = self.chunks.get_mut(index) else {
            return 0;
        };

        c.acked = true;
        c.retransmit = false;
        let n = c.user_data.len();
        c.user_data.clear();
        self.n_bytes -= n;
        n
    }

    pub(crate) fn mark_all_to_retrasmit(&mut self) {
        for c in &mut self.chunks {
            if c.acked || c.abandoned() {
                continue;
            }
            c.retransmit = true;
        }
    }

    pub(crate) fn get_num_bytes(&self) -> usize {
        self.n_bytes
    }

    pub(crate) fn len(&self) -> usize {
        self.chunks.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub(crate) fn last_tsn(&self) -> Option<u32> {
        self.chunks.back().map(|chunk| chunk.tsn)
    }
}
