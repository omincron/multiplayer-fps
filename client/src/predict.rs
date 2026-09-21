use common::config::{CLIENT_INPUT_HZ, INPUT_DT_MS, MAX_RTT_S, PLAYER_SPEED_UPS};
use common::maze::MazeGrid;
use common::sim::resolve_move;
use common::types::{Tick, Vec2};
use std::collections::VecDeque;

/// Must match `server::world::PLAYER_RADIUS`. Kept local until that server
/// value is promoted to the shared contract on `develop`.
const PLAYER_RADIUS: f32 = 0.25;
const RECONCILE_EPSILON: f32 = 0.001;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputCommand {
    pub input_tick: Tick,
    pub move_dir: Vec2,
    pub facing: f32,
    pub shoot: bool,
}

#[derive(Debug, Clone, Copy)]
struct HistoryEntry {
    input: InputCommand,
    predicted_pos_after: Vec2,
}

#[derive(Debug)]
pub struct Predictor {
    position: Vec2,
    next_input_tick: Tick,
    accumulated_s: f64,
    history: VecDeque<HistoryEntry>,
    history_capacity: usize,
}

impl Predictor {
    pub fn new(position: Vec2) -> Self {
        let history_capacity = ((CLIENT_INPUT_HZ as f32 * MAX_RTT_S).ceil() as usize).max(1);
        Self {
            position,
            next_input_tick: 1,
            accumulated_s: 0.0,
            history: VecDeque::with_capacity(history_capacity),
            history_capacity,
        }
    }

    pub fn position(&self) -> Vec2 {
        self.position
    }

    pub fn advance_frame(
        &mut self,
        frame_time_s: f32,
        move_dir: Vec2,
        facing: f32,
        shoot: bool,
        maze: &MazeGrid,
    ) -> Vec<InputCommand> {
        if frame_time_s.is_finite() && frame_time_s > 0.0 {
            self.accumulated_s += frame_time_s as f64;
        }

        let fixed_dt_s = (INPUT_DT_MS / 1000.0) as f64;
        let mut emitted = Vec::new();
        while self.accumulated_s + f64::EPSILON >= fixed_dt_s {
            self.accumulated_s -= fixed_dt_s;
            let input = InputCommand {
                input_tick: self.next_input_tick,
                move_dir,
                facing,
                shoot,
            };
            self.next_input_tick = self.next_input_tick.wrapping_add(1);
            self.position = apply_input(maze, self.position, input);
            self.history.push_back(HistoryEntry {
                input,
                predicted_pos_after: self.position,
            });
            if self.history.len() > self.history_capacity {
                self.history.pop_front();
            }
            emitted.push(input);
        }
        emitted
    }

    /// Returns the correction applied to the current predicted position.
    pub fn reconcile(
        &mut self,
        authoritative_pos: Vec2,
        last_input_tick: Tick,
        maze: &MazeGrid,
    ) -> Vec2 {
        let matches_acknowledged_prediction = self
            .history
            .iter()
            .find(|entry| entry.input.input_tick == last_input_tick)
            .is_some_and(|entry| {
                distance(entry.predicted_pos_after, authoritative_pos) <= RECONCILE_EPSILON
            });

        self.history
            .retain(|entry| entry.input.input_tick > last_input_tick);
        if matches_acknowledged_prediction {
            return Vec2::ZERO;
        }

        let previous_current = self.position;
        self.position = authoritative_pos;
        for entry in &mut self.history {
            self.position = apply_input(maze, self.position, entry.input);
            entry.predicted_pos_after = self.position;
        }
        Vec2::new(
            self.position.x - previous_current.x,
            self.position.y - previous_current.y,
        )
    }
}

fn apply_input(maze: &MazeGrid, position: Vec2, input: InputCommand) -> Vec2 {
    resolve_move(
        maze,
        position,
        input.move_dir,
        PLAYER_SPEED_UPS,
        INPUT_DT_MS / 1000.0,
        PLAYER_RADIUS,
    )
}

fn distance(a: Vec2, b: Vec2) -> f32 {
    (a.x - b.x).hypot(a.y - b.y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::config::INPUT_DT_MS;
    use common::maze::{MazeGrid, WALL_E, WALL_N, WALL_S, WALL_W};

    fn corridor(width: u16) -> MazeGrid {
        let mut walls = vec![WALL_N | WALL_S; width as usize];
        walls[0] |= WALL_W;
        walls[width as usize - 1] |= WALL_E;
        MazeGrid {
            width,
            height: 1,
            walls,
        }
    }

    #[test]
    fn emitted_input_moves_prediction_without_a_server_reply() {
        let maze = corridor(10);
        let mut predictor = Predictor::new(Vec2::new(2.5, 0.5));

        let inputs =
            predictor.advance_frame(INPUT_DT_MS / 1000.0, Vec2::new(1.0, 0.0), 0.0, false, &maze);

        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].input_tick, 1);
        assert!((predictor.position().x - 2.6).abs() < 0.001);
    }

    #[test]
    fn one_second_of_input_is_independent_of_render_frame_rate() {
        let maze = corridor(20);
        let at_30 = simulate_one_second(30, &maze);
        let at_120 = simulate_one_second(120, &maze);

        assert!((at_30.x - 5.5).abs() < 0.001);
        assert!((at_30.x - at_120.x).abs() < 0.001);
        assert!((at_30.y - at_120.y).abs() < 0.001);
    }

    #[test]
    fn matching_older_ack_has_zero_correction_after_newer_inputs() {
        let maze = corridor(20);
        let mut predictor = Predictor::new(Vec2::new(2.5, 0.5));
        let mut inputs = Vec::new();
        for _ in 0..5 {
            inputs.extend(predictor.advance_frame(
                INPUT_DT_MS / 1000.0,
                Vec2::new(1.0, 0.0),
                0.0,
                false,
                &maze,
            ));
        }
        let current_before = predictor.position();
        let authoritative_at_tick_two = predictor
            .history
            .iter()
            .find(|entry| entry.input.input_tick == 2)
            .unwrap()
            .predicted_pos_after;

        let correction = predictor.reconcile(authoritative_at_tick_two, 2, &maze);

        assert!(correction.x.abs() < 0.001 && correction.y.abs() < 0.001);
        assert_eq!(predictor.position(), current_before);
        assert!(
            predictor
                .history
                .iter()
                .all(|entry| entry.input.input_tick > 2)
        );
    }

    fn simulate_one_second(render_fps: usize, maze: &MazeGrid) -> Vec2 {
        let mut predictor = Predictor::new(Vec2::new(2.5, 0.5));
        for _ in 0..render_fps {
            predictor.advance_frame(
                1.0 / render_fps as f32,
                Vec2::new(1.0, 0.0),
                0.0,
                false,
                maze,
            );
        }
        predictor.position()
    }
}
