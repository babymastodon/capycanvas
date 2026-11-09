use std::ops::Range;

#[derive(Debug, PartialEq)]
pub enum Tick { Wait, Due { slot: usize, skipped: Range<usize>, actual: f64 }, Done { skipped: Range<usize> } }

pub struct Clock { slots: Vec<f64>, cutoff: f64, spacing: f64, next: usize, last: Option<f64> }
impl Clock {
    pub fn new(slots: Vec<f64>, cutoff: f64, spacing: f64) -> Self {
        assert!(cutoff.is_finite() && cutoff > 0.0 && spacing.is_finite() && spacing > 0.0);
        assert!(slots.iter().all(|slot| slot.is_finite() && *slot >= 0.0 && *slot < cutoff));
        assert!(slots.windows(2).all(|pair| pair[0] <= pair[1]));
        Self { slots, cutoff, spacing, next: 0, last: None }
    }
    pub fn next_deadline(&self) -> Option<f64> {
        self.slots.get(self.next).map(|slot| slot.max(self.last.map_or(0.0, |last| last + self.spacing)).min(self.cutoff))
    }
    pub fn take_due(&mut self, now: f64) -> Tick {
        if now >= self.cutoff { let skipped = self.next..self.slots.len(); self.next = self.slots.len(); return Tick::Done { skipped }; }
        if self.next_deadline().is_none_or(|deadline| now < deadline) { return Tick::Wait; }
        let end = self.slots.partition_point(|slot| *slot <= now);
        if end <= self.next { return Tick::Wait; }
        let slot = end - 1;
        let skipped = self.next..slot;
        self.next = end;
        self.last = Some(now);
        Tick::Due { slot, skipped, actual: now }
    }
}
pub fn nominal_hz(workload:&str,stream:&str)->f64 {
    if workload.starts_with("resize-") {120.0} else {match stream {"resize"=>120.0,"wheel"=>60.0,"click"=>2.5,"key"=>10.0,"touch"=>250.0,_=>240.0}}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_and_programmatic_resize_share_the_requested_input_frequency() {
        assert_eq!(nominal_hz("resize-native","mouse"),120.0);
        assert_eq!(nominal_hz("resize-programmatic","resize"),120.0);
        assert_eq!(nominal_hz("scrub-while-painting","mouse"),240.0);
        assert_eq!(nominal_hz("scrub-while-painting","pen"),240.0);
    }
    #[test]
    fn late_wakeup_skips_slots_and_never_creates_a_catchup_burst() {
        let mut clock = Clock::new((0..1440).map(|i| i as f64 / 240.0).collect(), 6.0, 1.0 / 240.0);
        assert_eq!(clock.take_due(0.0), Tick::Due { slot: 0, skipped: 0..0, actual: 0.0 });
        assert_eq!(clock.take_due(0.020), Tick::Due { slot: 4, skipped: 1..4, actual: 0.020 });
        assert_eq!(clock.take_due(0.020_040), Tick::Wait);
        assert_eq!(clock.take_due(0.020_080), Tick::Wait);
        assert!(clock.next_deadline().unwrap() >= 0.020 + 1.0 / 240.0);
        assert_eq!(clock.take_due(0.025), Tick::Due { slot: 6, skipped: 5..6, actual: 0.025 });
    }
    #[test]
    fn api_cost_does_not_add_an_extra_period_to_the_input_schedule() {
        let mut clock = Clock::new((0..1440).map(|i| i as f64 / 240.0).collect(), 6.0, 1.0 / 240.0);
        assert!(matches!(clock.take_due(0.0), Tick::Due { .. }));
        assert_eq!(clock.take_due(0.001), Tick::Wait);
        assert_eq!(clock.take_due(1.0 / 240.0), Tick::Due { slot: 1, skipped: 1..1, actual: 1.0 / 240.0 });
        assert_eq!(clock.take_due(0.020), Tick::Due { slot: 4, skipped: 2..4, actual: 0.020 });
        assert_eq!(clock.take_due(0.020_040), Tick::Wait);
    }
    #[test]
    fn six_second_cutoff_drops_remaining_slots_instead_of_extending_the_contact() {
        let mut clock = Clock::new((0..1440).map(|i| i as f64 / 240.0).collect(), 6.0, 1.0 / 240.0);
        assert!(matches!(clock.take_due(5.999), Tick::Due { slot: 1439, .. }));
        assert_eq!(clock.next_deadline(), None);
        assert_eq!(clock.take_due(6.001), Tick::Done { skipped: 1440..1440 });
        let mut clock = Clock::new(vec![0.0, 1.0, 2.0, 5.99], 6.0, 1.0);
        assert_eq!(clock.take_due(6.0), Tick::Done { skipped: 0..4 });
    }
    #[test]
    fn trajectory_uses_actual_injection_time_after_skipping() {
        let mut clock = Clock::new((0..1680).map(|i| i as f64 / 240.0).collect(), 7.0, 1.0 / 240.0);
        let Tick::Due { slot, actual, .. } = clock.take_due(1.234) else { panic!("missing due slot") };
        let point = crate::trajectory::drag(actual, 1.0, 500.0, 60.0);
        assert_eq!(point.value, crate::trajectory::drag(1.234, 1.0, 500.0, 60.0).value);
        assert_ne!(point.value, crate::trajectory::drag(slot as f64 / 240.0, 1.0, 500.0, 60.0).value);
    }
    #[test]
    fn simultaneous_streams_keep_independent_minimum_spacing() {
        let mut pen = Clock::new(vec![0.0, 0.004, 0.008], 6.0, 0.004);
        let mut mouse = Clock::new(vec![0.0, 0.004, 0.008], 6.0, 0.004);
        assert!(matches!(pen.take_due(0.006), Tick::Due { slot: 1, .. }));
        assert!(matches!(mouse.take_due(0.006_040), Tick::Due { slot: 1, .. }));
        assert_eq!(pen.take_due(0.008), Tick::Wait);
        assert_eq!(mouse.take_due(0.008), Tick::Wait);
    }
}
