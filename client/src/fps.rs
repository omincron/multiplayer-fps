use common::config::FPS_AVG_WINDOW_FRAMES;
use std::collections::VecDeque;

/// Smooths the displayed FPS over a bounded window of recent frame times.
#[derive(Debug)]
pub struct FpsMeter {
    frame_times_s: VecDeque<f32>,
    window_size: usize,
    total_time_s: f32,
}

impl FpsMeter {
    pub fn new(window_size: usize) -> Self {
        assert!(window_size > 0, "FPS averaging window must not be empty");
        Self {
            frame_times_s: VecDeque::with_capacity(window_size),
            window_size,
            total_time_s: 0.0,
        }
    }

    pub fn record_frame(&mut self, frame_time_s: f32) {
        if !frame_time_s.is_finite() || frame_time_s <= 0.0 {
            return;
        }

        self.frame_times_s.push_back(frame_time_s);
        self.total_time_s += frame_time_s;
        if self.frame_times_s.len() > self.window_size {
            self.total_time_s -= self.frame_times_s.pop_front().expect("window is non-empty");
        }
    }

    pub fn fps(&self) -> f32 {
        if self.frame_times_s.is_empty() || self.total_time_s <= 0.0 {
            return 0.0;
        }
        self.frame_times_s.len() as f32 / self.total_time_s
    }

    pub fn window_size(&self) -> usize {
        self.window_size
    }
}

impl Default for FpsMeter {
    fn default() -> Self {
        Self::new(FPS_AVG_WINDOW_FRAMES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_rolling_average_instead_of_last_instantaneous_frame() {
        let mut meter = FpsMeter::new(3);

        meter.record_frame(0.010);
        meter.record_frame(0.010);
        meter.record_frame(0.040);

        // Average frame time is 20 ms, hence 50 fps. Using only the last
        // frame would incorrectly report 25 fps.
        assert!((meter.fps() - 50.0).abs() < 0.001);
    }

    #[test]
    fn retains_only_the_configured_number_of_frames() {
        let mut meter = FpsMeter::new(2);
        meter.record_frame(0.100);
        meter.record_frame(0.010);
        meter.record_frame(0.010);

        assert!((meter.fps() - 100.0).abs() < 0.001);
    }

    #[test]
    fn production_meter_uses_the_shared_window_constant() {
        let meter = FpsMeter::default();
        assert_eq!(meter.window_size(), FPS_AVG_WINDOW_FRAMES);
    }
}
