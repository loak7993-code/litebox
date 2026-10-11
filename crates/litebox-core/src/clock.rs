//! Virtual monotonic clock. Real wall time never reaches app code: time only
//! moves when the scheduler advances it, which makes every run reproducible.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmClock {
    /// Virtual microseconds since VM boot.
    pub micros: u128,
}

impl VmClock {
    pub fn new() -> Self {
        VmClock { micros: 0 }
    }

    pub fn now_us(&self) -> u64 {
        self.micros.min(u128::from(u64::MAX)) as u64
    }

    pub fn millis(&self) -> u64 {
        (self.micros / 1000).min(u128::from(u64::MAX)) as u64
    }

    pub fn advance_us(&mut self, us: u64) {
        self.micros += us as u128;
    }

    pub fn advance_to(&mut self, us: u128) {
        if us > self.micros {
            self.micros = us;
        }
    }
}

impl Default for VmClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advances_and_saturates() {
        let mut c = VmClock::new();
        assert_eq!(c.now_us(), 0);
        c.advance_us(1_500);
        assert_eq!(c.now_us(), 1_500);
        assert_eq!(c.millis(), 1);
        c.advance_to(1_000);
        assert_eq!(c.now_us(), 1_500, "advance_to never rewinds");
        c.micros = u128::MAX;
        assert_eq!(c.now_us(), u64::MAX);
    }
}
