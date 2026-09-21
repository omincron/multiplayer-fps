use common::config::{MAX_EXTRAPOLATION_MS, SERVER_TICK_HZ, SNAPSHOT_BUFFER_LEN};
use common::types::{Tick, Vec2};
use std::cmp::Ordering;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimedSnapshot {
    pub server_time_ms: f64,
    pub pos: Vec2,
    pub facing: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleMode {
    Interpolated,
    Extrapolated,
    Held,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SampledState {
    pub pos: Vec2,
    pub facing: f32,
    pub mode: SampleMode,
}

#[derive(Debug, Default)]
pub struct SnapshotBuffer {
    snapshots: Vec<TimedSnapshot>,
}

impl SnapshotBuffer {
    pub fn push(&mut self, snapshot: TimedSnapshot) {
        match self.snapshots.binary_search_by(|existing| {
            existing
                .server_time_ms
                .partial_cmp(&snapshot.server_time_ms)
                .unwrap_or(Ordering::Equal)
        }) {
            Ok(index) => self.snapshots[index] = snapshot,
            Err(index) => self.snapshots.insert(index, snapshot),
        }
        if self.snapshots.len() > SNAPSHOT_BUFFER_LEN {
            self.snapshots.remove(0);
        }
    }

    pub fn len(&self) -> usize {
        self.snapshots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }

    /// Drops all buffered snapshots. Used on `Respawned` (Milestone 13): a
    /// respawn is a teleport, not movement, so interpolating from the
    /// pre-respawn snapshots to the new position would render a visible
    /// slide across the map instead of an instant pop.
    pub fn clear(&mut self) {
        self.snapshots.clear();
    }

    pub fn sample(&self, target_server_time_ms: f64) -> Option<SampledState> {
        let first = *self.snapshots.first()?;
        if target_server_time_ms < first.server_time_ms {
            return Some(held(first));
        }

        for pair in self.snapshots.windows(2) {
            let (older, newer) = (pair[0], pair[1]);
            if target_server_time_ms <= newer.server_time_ms {
                let span = newer.server_time_ms - older.server_time_ms;
                let amount = if span > 0.0 {
                    ((target_server_time_ms - older.server_time_ms) / span) as f32
                } else {
                    0.0
                };
                return Some(SampledState {
                    pos: lerp_vec(older.pos, newer.pos, amount),
                    facing: lerp_angle(older.facing, newer.facing, amount),
                    mode: SampleMode::Interpolated,
                });
            }
        }

        let latest = *self.snapshots.last()?;
        let beyond_ms = target_server_time_ms - latest.server_time_ms;
        if beyond_ms <= MAX_EXTRAPOLATION_MS as f64
            && let Some(previous) = self.snapshots.iter().rev().nth(1).copied()
        {
            let span_ms = latest.server_time_ms - previous.server_time_ms;
            if span_ms > 0.0 {
                let factor = (beyond_ms / span_ms) as f32;
                return Some(SampledState {
                    pos: Vec2::new(
                        latest.pos.x + (latest.pos.x - previous.pos.x) * factor,
                        latest.pos.y + (latest.pos.y - previous.pos.y) * factor,
                    ),
                    facing: normalize_angle(
                        latest.facing + shortest_angle(previous.facing, latest.facing) * factor,
                    ),
                    mode: SampleMode::Extrapolated,
                });
            }
        }
        Some(held(latest))
    }
}

fn held(snapshot: TimedSnapshot) -> SampledState {
    SampledState {
        pos: snapshot.pos,
        facing: snapshot.facing,
        mode: SampleMode::Held,
    }
}

fn lerp_vec(a: Vec2, b: Vec2, amount: f32) -> Vec2 {
    Vec2::new(a.x + (b.x - a.x) * amount, a.y + (b.y - a.y) * amount)
}

fn lerp_angle(a: f32, b: f32, amount: f32) -> f32 {
    normalize_angle(a + shortest_angle(a, b) * amount)
}

fn shortest_angle(from: f32, to: f32) -> f32 {
    normalize_angle(to - from)
}

fn normalize_angle(angle: f32) -> f32 {
    let full_turn = std::f32::consts::TAU;
    (angle + std::f32::consts::PI).rem_euclid(full_turn) - std::f32::consts::PI
}

const CLOCK_SAMPLE_CAPACITY: usize = 8;

#[derive(Debug, Default)]
pub struct ClockSync {
    samples: VecDeque<(f64, f64)>,
}

impl ClockSync {
    pub fn observe_pong(
        &mut self,
        ping_sent_local_ms: f64,
        pong_received_local_ms: f64,
        server_time_ms: f64,
    ) {
        let rtt_ms = (pong_received_local_ms - ping_sent_local_ms).max(0.0);
        let offset_ms = server_time_ms + rtt_ms * 0.5 - pong_received_local_ms;
        self.samples.push_back((rtt_ms, offset_ms));
        if self.samples.len() > CLOCK_SAMPLE_CAPACITY {
            self.samples.pop_front();
        }
    }

    pub fn offset_ms(&self) -> Option<f64> {
        self.samples
            .iter()
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal))
            .map(|sample| sample.1)
    }

    pub fn server_time_ms(&self, local_time_ms: f64) -> Option<f64> {
        self.offset_ms().map(|offset| local_time_ms + offset)
    }
}

pub fn tick_to_server_time_ms(tick: Tick, anchor_tick: Tick, anchor_server_time_ms: u64) -> f64 {
    let tick_delta = tick.wrapping_sub(anchor_tick) as i32 as f64;
    anchor_server_time_ms as f64 + tick_delta * 1000.0 / SERVER_TICK_HZ as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::config::{INTERP_DELAY_MS, SERVER_TICK_HZ, SNAPSHOT_BUFFER_LEN};
    use common::types::Vec2;

    fn sample(time_ms: f64, x: f32) -> TimedSnapshot {
        TimedSnapshot {
            server_time_ms: time_ms,
            pos: Vec2::new(x, 0.0),
            facing: 0.0,
        }
    }

    #[test]
    fn interpolates_between_bracketing_snapshots() {
        let mut buffer = SnapshotBuffer::default();
        buffer.push(sample(100.0, 1.0));
        buffer.push(sample(200.0, 3.0));

        let result = buffer.sample(150.0).unwrap();

        assert_eq!(result.mode, SampleMode::Interpolated);
        assert!((result.pos.x - 2.0).abs() < 0.001);
    }

    #[test]
    fn extrapolates_briefly_then_holds_latest_position() {
        let mut buffer = SnapshotBuffer::default();
        buffer.push(sample(100.0, 1.0));
        buffer.push(sample(200.0, 2.0));

        let extrapolated = buffer.sample(250.0).unwrap();
        assert_eq!(extrapolated.mode, SampleMode::Extrapolated);
        assert!((extrapolated.pos.x - 2.5).abs() < 0.001);

        let held = buffer.sample(500.0).unwrap();
        assert_eq!(held.mode, SampleMode::Held);
        assert!((held.pos.x - 2.0).abs() < 0.001);
    }

    #[test]
    fn configured_buffer_brackets_the_delayed_render_target() {
        let mut buffer = SnapshotBuffer::default();
        let tick_ms = 1000.0 / SERVER_TICK_HZ as f32;
        for tick in 0..=SERVER_TICK_HZ {
            buffer.push(sample((tick as f32 * tick_ms) as f64, tick as f32));
        }

        assert_eq!(buffer.len(), SNAPSHOT_BUFFER_LEN);
        let result = buffer.sample((1000.0 - INTERP_DELAY_MS) as f64).unwrap();
        assert_eq!(result.mode, SampleMode::Interpolated);
    }

    #[test]
    fn interpolates_facing_across_the_short_angle_path() {
        let mut buffer = SnapshotBuffer::default();
        let mut first = sample(100.0, 0.0);
        first.facing = 170_f32.to_radians();
        let mut second = sample(200.0, 0.0);
        second.facing = (-170_f32).to_radians();
        buffer.push(first);
        buffer.push(second);

        let result = buffer.sample(150.0).unwrap();
        assert!((result.facing.abs() - std::f32::consts::PI).abs() < 0.001);
    }

    #[test]
    fn clock_estimate_ignores_a_badly_delayed_outlier() {
        let mut clock = ClockSync::default();
        clock.observe_pong(900.0, 1100.0, 1050.0);
        let baseline = clock.offset_ms().unwrap();

        clock.observe_pong(1200.0, 2200.0, 1350.0);

        assert!((clock.offset_ms().unwrap() - baseline).abs() < 0.001);
    }

    #[test]
    fn tick_time_uses_the_server_clock_anchor() {
        assert!((tick_to_server_time_ms(103, 100, 50_000) - 50_100.0).abs() < 0.001);
        assert!((tick_to_server_time_ms(97, 100, 50_000) - 49_900.0).abs() < 0.001);
    }
}
