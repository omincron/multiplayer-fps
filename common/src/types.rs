//! Small shared value types used across the protocol and simulation
//! (ARCHITECTURE.md §2).

use serde::{Deserialize, Serialize};

pub type PlayerId = u32;
pub type Tick = u32;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub fn new(x: f32, y: f32) -> Self {
        Vec2 { x, y }
    }
}
