/// Association statistics
#[derive(Default, Debug, Copy, Clone)]
pub struct AssociationStats {
    n_datas: u64,
    n_sacks: u64,
    n_t3timeouts: u64,
    n_ack_timeouts: u64,
    n_rack_loss_marks: u64,
    n_pto_timeouts: u64,
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
