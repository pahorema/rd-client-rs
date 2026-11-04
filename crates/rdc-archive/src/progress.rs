//! Byte-based extraction progress reported as a whole percentage.

/// Tracks extracted bytes against the total and reports each new percentage once.
pub(crate) struct Progress<'a> {
    total: u64,
    done: u64,
    reported: Option<u8>,
    sink: &'a mut (dyn FnMut(u8) + Send),
}

impl<'a> Progress<'a> {
    /// Starts tracking `total` bytes and reports 0 %.
    pub(crate) fn new(total: u64, on_progress: &'a mut (dyn FnMut(u8) + Send)) -> Self {
        let mut progress = Self {
            total,
            done: 0,
            reported: None,
            sink: on_progress,
        };
        progress.report();
        progress
    }

    /// Bytes counted so far.
    pub(crate) fn done(&self) -> u64 {
        self.done
    }

    /// Counts `bytes` more extracted bytes.
    pub(crate) fn advance(&mut self, bytes: u64) {
        self.advance_to(self.done.saturating_add(bytes));
    }

    /// Moves the counter forward to `done` bytes; never moves it backwards.
    pub(crate) fn advance_to(&mut self, done: u64) {
        if done > self.done {
            self.done = done;
            self.report();
        }
    }

    /// Reports 100 %, whatever the byte count.
    pub(crate) fn finish(&mut self) {
        self.emit(100);
    }

    fn report(&mut self) {
        let percent = if self.total == 0 {
            0
        } else {
            let ratio = u128::from(self.done.min(self.total)) * 100 / u128::from(self.total);
            u8::try_from(ratio).unwrap_or(100)
        };
        self.emit(percent);
    }

    fn emit(&mut self, percent: u8) {
        if self.reported.is_none_or(|reported| percent > reported) {
            self.reported = Some(percent);
            (self.sink)(percent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_percentage_is_reported_once_and_in_order() {
        let mut seen = Vec::new();
        let mut record = |percent| seen.push(percent);
        let mut progress = Progress::new(200, &mut record);
        progress.advance(1);
        progress.advance(1);
        progress.advance(99);
        progress.advance_to(50);
        progress.advance(500);
        progress.finish();

        assert_eq!(seen, [0, 1, 50, 100]);
    }

    #[test]
    fn an_empty_total_jumps_from_zero_to_one_hundred() {
        let mut seen = Vec::new();
        let mut record = |percent| seen.push(percent);
        let mut progress = Progress::new(0, &mut record);
        progress.advance(10);
        progress.finish();

        assert_eq!(seen, [0, 100]);
    }
}
