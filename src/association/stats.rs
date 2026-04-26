/// Association statistics
#[derive(Default, Debug, Copy, Clone)]
pub struct AssociationStats {
    pub(crate) n_datas: u64,
    pub(crate) n_sacks: u64,
    pub(crate) n_t3timeouts: u64,
    pub(crate) n_ack_timeouts: u64,
    pub(crate) n_rack_loss_marks: u64,
    pub(crate) n_pto_timeouts: u64,
}

impl AssociationStats {
    pub fn inc_datas(&mut self) {
        self.n_datas += 1;
    }

    pub fn get_num_datas(&mut self) -> u64 {
        self.n_datas
    }

    pub fn inc_sacks(&mut self) {
        self.n_sacks += 1;
    }

    pub fn get_num_sacks(&mut self) -> u64 {
        self.n_sacks
    }

    pub fn inc_t3timeouts(&mut self) {
        self.n_t3timeouts += 1;
    }

    pub fn get_num_t3timeouts(&mut self) -> u64 {
        self.n_t3timeouts
    }

    pub fn inc_ack_timeouts(&mut self) {
        self.n_ack_timeouts += 1;
    }

    pub fn get_num_ack_timeouts(&mut self) -> u64 {
        self.n_ack_timeouts
    }

    pub fn inc_rack_loss_marks(&mut self) {
        self.n_rack_loss_marks += 1;
    }

    pub fn get_num_rack_loss_marks(&mut self) -> u64 {
        self.n_rack_loss_marks
    }

    pub fn inc_pto_timeouts(&mut self) {
        self.n_pto_timeouts += 1;
    }

    pub fn get_num_pto_timeouts(&mut self) -> u64 {
        self.n_pto_timeouts
    }

    pub fn reset(&mut self) {
        self.n_datas = 0;
        self.n_sacks = 0;
        self.n_t3timeouts = 0;
        self.n_ack_timeouts = 0;
        self.n_rack_loss_marks = 0;
        self.n_pto_timeouts = 0;
    }
}

#[derive(Default, Debug, Copy, Clone)]
pub struct AssociationSnapshot {
    pub n_datas: u64,
    pub n_sacks: u64,
    pub n_t3timeouts: u64,
    pub n_ack_timeouts: u64,
    pub n_rack_loss_marks: u64,
    pub n_pto_timeouts: u64,
    pub cwnd: u32,
    pub ssthresh: u32,
    pub rwnd: u32,
    pub in_fast_recovery: bool,
    pub srtt_ms: u64,
    pub rto_ms: u64,
    pub inflight_bytes: u64,
    pub inflight_chunks: u64,
    pub pending_bytes: u64,
    pub pending_chunks: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub rack_reo_wnd_us: u64,
    pub rack_min_rtt_us: u64,
    pub rack_jitter_p95_us: u64,
    pub rack_jitter_sample_count: u32,
}
