use std::collections::VecDeque;

#[derive(Debug)]
pub struct Limiter {
    window_secs: u64,
    max: usize,
    stamps: VecDeque<u64>,
}

impl Limiter {
    pub fn new(window_secs: u64, max: usize) -> Self {
        Self {
            window_secs,
            max,
            stamps: VecDeque::new(),
        }
    }

    pub fn allow(&mut self, now: u64) -> bool {
        let cutoff = now.saturating_sub(self.window_secs);
        while self.stamps.front().is_some_and(|t| *t <= cutoff) {
            self.stamps.pop_front();
        }
        if self.stamps.len() >= self.max {
            return false;
        }
        self.stamps.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::Limiter;

    #[test]
    fn allows_up_to_max_then_blocks() {
        let mut lim = Limiter::new(20, 4);
        for i in 0..4 {
            assert!(lim.allow(100 + i), "hit {i}");
        }
        assert!(!lim.allow(104));
        assert!(lim.allow(121));
    }

    #[test]
    fn slash_window_is_eight() {
        let mut lim = Limiter::new(20, 8);
        for i in 0..8 {
            assert!(lim.allow(50 + i));
        }
        assert!(!lim.allow(60));
        assert!(lim.allow(71));
    }
}
