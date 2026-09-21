//! Player-sprite billboard projection (ARCHITECTURE.md §6.5: "simple flat
//! polygon/wireframe billboards, scaled by distance, drawn only for
//! players within line-of-sight per the same depth buffer" as the wall
//! raycaster). Pure math only — no drawing here, same split as
//! `render::raycast` (pure DDA math) vs. `main.rs` (the actual
//! `draw_rectangle` calls), so the projection can be unit-tested against
//! hand-computed expected values the way the raycaster already is.

use std::ops::Range;

use common::types::Vec2;

use super::raycast::ViewColumn;

/// A sprite's on-screen anchor, in the same units `render::raycast`
/// already uses: `depth` is the perpendicular (fisheye-corrected)
/// distance, directly comparable to `ViewColumn::depth`, and
/// `screen_height` is the same "apparent size at this depth" the wall
/// renderer computes (`projection_plane / depth`) — a full floor-to-
/// ceiling scale, which the caller then applies a smaller ratio to for an
/// actual human-sized silhouette rather than a wall-sized one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteProjection {
    pub depth: f32,
    pub screen_x: f32,
    pub screen_height: f32,
}

/// Half the field of view's worth of extra angular margin: a sprite whose
/// *center* is just outside the FOV can still have half its width poking
/// into view, so cutting off exactly at the FOV edge would pop it in/out
/// abruptly as it crosses. Generous on purpose — projecting a sprite that
/// turns out to be fully off-screen is cheap; missing one that should be
/// partially visible is a visible glitch.
const SPRITE_ANGULAR_MARGIN: f32 = 0.3;

/// Projects `sprite_pos` into screen space for a camera at `camera_pos`
/// facing `camera_facing`. Returns `None` if the sprite is behind the
/// camera plane or far enough outside the field of view that no part of
/// it could be visible. `projection_plane` is passed in rather than
/// recomputed from `screen_width`/`field_of_view` so wall and sprite
/// sizing can never drift apart by using two slightly different formulas.
pub fn project_sprite(
    camera_pos: Vec2,
    camera_facing: f32,
    field_of_view: f32,
    screen_width: f32,
    projection_plane: f32,
    sprite_pos: Vec2,
) -> Option<SpriteProjection> {
    let dx = sprite_pos.x - camera_pos.x;
    let dy = sprite_pos.y - camera_pos.y;
    let raw_distance = dx.hypot(dy);
    if raw_distance <= f32::EPSILON {
        return None; // exactly on top of the camera: nothing sane to draw
    }

    let angle_to_sprite = dy.atan2(dx);
    let angle_delta = normalize_angle(angle_to_sprite - camera_facing);
    let half_fov = field_of_view * 0.5;
    if angle_delta.abs() > half_fov + SPRITE_ANGULAR_MARGIN {
        return None;
    }

    // Same correction as `raycast::perpendicular_distance`, applied to the
    // sprite's own bearing instead of a ray's — keeps sprite depth and
    // wall depth on one consistent scale.
    let depth = raw_distance * angle_delta.cos();
    if depth <= 0.0 {
        return None; // behind the camera plane
    }

    let fraction = (angle_delta + half_fov) / field_of_view;
    Some(SpriteProjection {
        depth,
        screen_x: fraction * screen_width,
        screen_height: projection_plane / depth,
    })
}

/// The screen columns a sprite's silhouette (of the given fraction of its
/// full `screen_height` as width) spans, clamped to `[0, columns)`. Purely
/// a range to *attempt* — per-column occlusion (`is_occluded`) still needs
/// to be applied to each one against that column's actual wall depth.
pub fn sprite_column_range(
    projection: &SpriteProjection,
    columns: usize,
    width_ratio: f32,
) -> Range<usize> {
    let half_width = projection.screen_height * width_ratio * 0.5;
    let start = (projection.screen_x - half_width).floor().max(0.0) as usize;
    let end = ((projection.screen_x + half_width).ceil() as isize).clamp(0, columns as isize);
    let end = end as usize;
    start.min(end)..end
}

/// Whether the wall already drawn in this column is nearer than the
/// sprite, and therefore hides it — the exact mechanism ARCHITECTURE.md
/// §6.5 calls for reusing the wall depth buffer for. A column with no
/// wall hit at all (`None`, i.e. the ray reached `max_distance` without
/// finding one) occludes nothing.
pub fn is_occluded(projection: &SpriteProjection, wall_column: Option<ViewColumn>) -> bool {
    match wall_column {
        Some(wall) => wall.depth < projection.depth,
        None => false,
    }
}

fn normalize_angle(angle: f32) -> f32 {
    let full_turn = std::f32::consts::TAU;
    (angle + std::f32::consts::PI).rem_euclid(full_turn) - std::f32::consts::PI
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::raycast::WallSide;

    const FOV: f32 = std::f32::consts::FRAC_PI_2; // 90 degrees, easy fractions
    const SCREEN_WIDTH: f32 = 800.0;
    const PROJECTION_PLANE: f32 = 400.0;

    #[test]
    fn sprite_dead_ahead_projects_to_the_screen_center() {
        let projection = project_sprite(
            Vec2::new(0.0, 0.0),
            0.0,
            FOV,
            SCREEN_WIDTH,
            PROJECTION_PLANE,
            Vec2::new(5.0, 0.0),
        )
        .expect("dead ahead sprite should project");

        assert!((projection.depth - 5.0).abs() < 0.001);
        assert!((projection.screen_x - SCREEN_WIDTH / 2.0).abs() < 0.001);
        assert!((projection.screen_height - PROJECTION_PLANE / 5.0).abs() < 0.001);
    }

    #[test]
    fn sprite_at_the_left_fov_edge_projects_near_column_zero() {
        // Exactly `half_fov` to the camera's left (camera facing "east",
        // angle 0.0) — the left screen edge.
        let half_fov = FOV / 2.0;
        let sprite_pos = Vec2::new(5.0 * (-half_fov).cos(), 5.0 * (-half_fov).sin());

        let projection = project_sprite(
            Vec2::new(0.0, 0.0),
            0.0,
            FOV,
            SCREEN_WIDTH,
            PROJECTION_PLANE,
            sprite_pos,
        )
        .expect("sprite at the FOV edge should still project");

        assert!(projection.screen_x.abs() < 1.0);
    }

    #[test]
    fn sprite_behind_the_camera_does_not_project() {
        let projection = project_sprite(
            Vec2::new(0.0, 0.0),
            0.0,
            FOV,
            SCREEN_WIDTH,
            PROJECTION_PLANE,
            Vec2::new(-5.0, 0.0),
        );
        assert!(projection.is_none());
    }

    #[test]
    fn sprite_well_outside_the_field_of_view_does_not_project() {
        // 90 degrees to the camera's left, well past FOV/2 + margin.
        let projection = project_sprite(
            Vec2::new(0.0, 0.0),
            0.0,
            FOV,
            SCREEN_WIDTH,
            PROJECTION_PLANE,
            Vec2::new(0.0, 5.0).rotate_around_origin(std::f32::consts::FRAC_PI_2),
        );
        assert!(projection.is_none());
    }

    #[test]
    fn nearer_wall_occludes_a_farther_sprite() {
        let projection = SpriteProjection {
            depth: 10.0,
            screen_x: 400.0,
            screen_height: 40.0,
        };
        let nearer_wall = Some(ViewColumn {
            depth: 5.0,
            side: WallSide::Vertical,
        });
        assert!(is_occluded(&projection, nearer_wall));
    }

    #[test]
    fn a_sprite_nearer_than_the_wall_is_not_occluded() {
        let projection = SpriteProjection {
            depth: 5.0,
            screen_x: 400.0,
            screen_height: 80.0,
        };
        let farther_wall = Some(ViewColumn {
            depth: 10.0,
            side: WallSide::Vertical,
        });
        assert!(!is_occluded(&projection, farther_wall));
    }

    #[test]
    fn no_wall_hit_in_a_column_never_occludes() {
        let projection = SpriteProjection {
            depth: 5.0,
            screen_x: 400.0,
            screen_height: 80.0,
        };
        assert!(!is_occluded(&projection, None));
    }

    #[test]
    fn column_range_is_centered_on_screen_x_and_clamped_to_bounds() {
        let projection = SpriteProjection {
            depth: 5.0,
            screen_x: 10.0,
            screen_height: 100.0, // half-width at ratio 0.5 = 25.0
        };
        let range = sprite_column_range(&projection, 800, 0.5);
        assert_eq!(range.start, 0, "should clamp to 0, not go negative");
        assert!(range.end > 0 && range.end < 800);
    }

    /// Small local test-only helper — rotates a point around the origin.
    /// Not worth adding to `common::types::Vec2` for one test's sake.
    trait RotateAroundOrigin {
        fn rotate_around_origin(self, angle: f32) -> Vec2;
    }
    impl RotateAroundOrigin for Vec2 {
        fn rotate_around_origin(self, angle: f32) -> Vec2 {
            Vec2::new(
                self.x * angle.cos() - self.y * angle.sin(),
                self.x * angle.sin() + self.y * angle.cos(),
            )
        }
    }
}
