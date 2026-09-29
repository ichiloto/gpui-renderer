//! Presentation-only slides of retained field sprites and of the camera that
//! follows one of them (`field_motion`).
//!
//! PHP commits every step and owns all timing that matters to play: a
//! sprite's cell is already where PHP put it. When a sprite's world cell
//! changes by one step and the sprite carries a motion hint, the renderer
//! draws it sliding from its previous cell over the hinted duration, on its
//! own clock. When the world origin changes in the same frame that the
//! followed sprite starts a slide, the camera slides with the same start and
//! duration, so the followed character stays steady while the field scrolls.
//! Anything else snaps, exactly as without `field_motion`.
use crate::retained_protocol::Viewport;
use crate::retained_state::SceneSource;
use std::collections::HashMap;

/// A step that arrives this close to the end of the one before it continues
/// that slide seamlessly, instead of starting over from where it is drawn.
/// Steps are sent as PHP commits them, so arrival varies by a frame or so.
pub const CHAIN_LEAD_SECONDS: f64 = 0.05;

type Cell = (f64, f64);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Slide {
    from: Cell,
    to: Cell,
    start: f64,
    duration: f64,
}

impl Slide {
    fn end(&self) -> f64 {
        self.start + self.duration
    }

    fn position(&self, now: f64) -> Cell {
        if self.duration <= 0.0 || now >= self.end() {
            return self.to;
        }
        if now <= self.start {
            return self.from;
        }
        let progress = (now - self.start) / self.duration;
        (
            self.from.0 + (self.to.0 - self.from.0) * progress,
            self.from.1 + (self.to.1 - self.from.1) * progress,
        )
    }
}

/// Where one subject is drawn, in world cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Track {
    target: Cell,
    current: Option<Slide>,
    /// The slide still drawn until `current` starts, when a step was chained.
    previous: Option<Slide>,
}

impl Track {
    pub fn new(target: Cell) -> Self {
        Self {
            target,
            current: None,
            previous: None,
        }
    }

    pub fn position(&self, now: f64) -> Cell {
        match (self.previous, self.current) {
            (Some(previous), Some(current)) if now < current.start => previous.position(now),
            (_, Some(current)) => current.position(now),
            _ => self.target,
        }
    }

    pub fn is_moving(&self, now: f64) -> bool {
        self.current.is_some_and(|current| now < current.end())
    }

    fn snap(&mut self, target: Cell) {
        *self = Self::new(target);
    }

    /// Slide to `target` over `duration` seconds and return when the slide
    /// starts: when the previous slide ends, if that is imminent, otherwise now.
    fn slide(&mut self, target: Cell, duration: f64, now: f64) -> f64 {
        let start = match self.current {
            Some(current)
                if current.start <= now
                    && current.end() >= now
                    && current.end() - now <= CHAIN_LEAD_SECONDS =>
            {
                current.end()
            }
            _ => now,
        };
        self.slide_from(target, start, duration, now);
        start
    }

    /// Slide to `target` from wherever this track is drawn at `start`.
    fn slide_from(&mut self, target: Cell, start: f64, duration: f64, now: f64) {
        let start = start.max(now);
        let from = self.position(start);
        self.previous = self
            .current
            .filter(|current| start > now && current.end() > now);
        self.current = Some(Slide {
            from,
            to: target,
            start,
            duration,
        });
        self.target = target;
    }
}

/// One cell in either direction or both: a step, not a placement.
fn is_step(from: Cell, to: Cell) -> bool {
    (to.0 - from.0).abs() <= 1.0 && (to.1 - from.1).abs() <= 1.0
}

/// Sprite and camera slides of the visible retained field.
#[derive(Debug, Default)]
pub struct FieldMotion {
    world_id: Option<String>,
    camera: Option<Track>,
    follow: Option<String>,
    sprites: HashMap<String, Track>,
}

impl FieldMotion {
    /// Observe a presented retained frame at `now` seconds.
    pub fn observe(&mut self, source: &SceneSource, viewport: Option<&Viewport>, now: f64) {
        let Some(view) = viewport.filter(|view| view.world_id.is_some()) else {
            *self = Self::default();
            return;
        };
        if self.world_id != view.world_id {
            *self = Self {
                world_id: view.world_id.clone(),
                ..Self::default()
            };
        }
        let origin = (
            f64::from(view.world_origin.column),
            f64::from(view.world_origin.row),
        );
        let follow = view.follow.as_ref().map(|follow| follow.sprite_id.clone());
        let mut followed_slide = None;
        let mut sprites = HashMap::with_capacity(view.sprite_ids.len());
        for id in &view.sprite_ids {
            let Some(sprite) = source.sprites.get(id) else {
                continue;
            };
            // Sprites are sent in camera-screen cells; the world cell is what moves.
            let target = (
                f64::from(sprite.item.x) + origin.0,
                f64::from(sprite.item.y) + origin.1,
            );
            let mut track = self
                .sprites
                .remove(id)
                .unwrap_or_else(|| Track::new(target));
            if track.target != target {
                match source.sprite_motions.get(id) {
                    Some(motion) if is_step(track.target, target) => {
                        let start = track.slide(target, motion.duration, now);
                        if follow.as_ref() == Some(id) {
                            followed_slide = Some((start, motion.duration));
                        }
                    }
                    _ => track.snap(target),
                }
            }
            sprites.insert(id.clone(), track);
        }
        self.sprites = sprites;
        let mut camera = self.camera.unwrap_or_else(|| Track::new(origin));
        if camera.target != origin {
            match followed_slide {
                Some((start, duration)) if is_step(camera.target, origin) => {
                    camera.slide_from(origin, start, duration, now);
                }
                _ => camera.snap(origin),
            }
        }
        self.camera = Some(camera);
        self.follow = follow;
    }

    pub fn is_animating(&self, now: f64) -> bool {
        self.camera.is_some_and(|camera| camera.is_moving(now))
            || self.sprites.values().any(|track| track.is_moving(now))
    }

    /// The camera's drawn world origin, in cells.
    pub fn get_camera_origin(&self, now: f64) -> Option<Cell> {
        self.camera.map(|camera| camera.position(now))
    }

    /// Cells to shift camera-screen content by: the camera's target origin
    /// minus where it is drawn.
    pub fn get_camera_shift(&self, now: f64) -> (f32, f32) {
        self.camera.map_or((0.0, 0.0), |camera| {
            let drawn = camera.position(now);
            (
                (camera.target.0 - drawn.0) as f32,
                (camera.target.1 - drawn.1) as f32,
            )
        })
    }

    /// Cells to shift a sprite by from the camera-screen cell it was sent at.
    pub fn get_sprite_shift(&self, id: &str, now: f64) -> (f32, f32) {
        let camera = self.get_camera_shift(now);
        self.sprites.get(id).map_or(camera, |track| {
            let drawn = track.position(now);
            (
                camera.0 + (drawn.0 - track.target.0) as f32,
                camera.1 + (drawn.1 - track.target.1) as f32,
            )
        })
    }

    /// Cells to shift a text layer by: with the followed sprite when it is
    /// anchored to it, otherwise with the field.
    pub fn get_text_shift(&self, view: &Viewport, id: &str, now: f64) -> (f32, f32) {
        match (&view.follow, &self.follow) {
            (Some(follow), Some(sprite))
                if follow.text_layer_ids.iter().any(|layer| layer == id) =>
            {
                self.get_sprite_shift(sprite, now)
            }
            _ => self.get_camera_shift(now),
        }
    }

    /// The viewport the world is painted through: whole cells from the drawn
    /// origin's floor, and the remaining fraction as a logical-pixel shift.
    pub fn get_world_viewport(&self, view: &Viewport, cell: (f32, f32), now: f64) -> Viewport {
        let Some(drawn) = self.get_camera_origin(now) else {
            return view.clone();
        };
        let whole = (drawn.0.floor(), drawn.1.floor());
        let mut world = view.clone();
        world.world_origin.column = whole.0 as i32;
        world.world_origin.row = whole.1 as i32;
        world.origin.x -= ((drawn.0 - whole.0) as f32) * cell.0 * view.scale;
        world.origin.y -= ((drawn.1 - whole.1) as f32) * cell.1 * view.scale;
        world
    }
}

/// The same viewport with its content moved by `cells` field cells.
pub fn shift_viewport(view: &Viewport, cells: (f32, f32), cell: (f32, f32)) -> Viewport {
    let mut shifted = view.clone();
    shifted.origin.x += cells.0 * cell.0 * view.scale;
    shifted.origin.y += cells.1 * cell.1 * view.scale;
    shifted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Grid;
    use crate::retained_protocol::FrameUpdate;
    use crate::retained_state::RetainedSession;
    use serde_json::{Value, json};

    const VERTICAL: f64 = 16.0 / 60.0;
    const HORIZONTAL: f64 = 8.0 / 60.0;

    fn close(a: Cell, b: Cell) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn a_slide_interpolates_linearly_and_ends_on_its_cell() {
        let mut track = Track::new((4.0, 4.0));
        let start = track.slide((4.0, 5.0), VERTICAL, 10.0);
        assert_eq!(start, 10.0);
        assert!(close(track.position(10.0), (4.0, 4.0)));
        assert!(close(track.position(10.0 + VERTICAL / 4.0), (4.0, 4.25)));
        assert!(close(track.position(10.0 + VERTICAL / 2.0), (4.0, 4.5)));
        assert!(close(track.position(10.0 + VERTICAL), (4.0, 5.0)));
        assert!(close(track.position(99.0), (4.0, 5.0)));
        assert!(track.is_moving(10.0) && !track.is_moving(10.0 + VERTICAL));
        track.snap((9.0, 9.0));
        assert!(!track.is_moving(10.0) && close(track.position(10.0), (9.0, 9.0)));
    }

    #[test]
    fn chained_steps_keep_one_even_speed_whatever_their_arrival_jitter() {
        // Arrivals a few milliseconds late, early and on time, as PHP's
        // update cadence and the pipe deliver them.
        let jitter = [0.0, 0.004, -0.003, 0.002, -0.004, 0.0, 0.003];
        let mut track = Track::new((0.0, 0.0));
        let mut starts = vec![];
        let mut arrivals = vec![];
        let mut samples = vec![];
        let mut next = 0;
        let mut now = 0.0;
        while now < 2.0 {
            let arrival = 0.1 + next as f64 * HORIZONTAL + jitter[next % jitter.len()];
            if next < 12 && now >= arrival {
                arrivals.push(now);
                starts.push(track.slide((next as f64 + 1.0, 0.0), HORIZONTAL, now));
                next += 1;
            }
            samples.push((now, track.position(now).0));
            now += 0.001;
        }
        assert_eq!(starts.len(), 12);
        // Once walking, each step starts exactly where the one before ended,
        // so the drawn position never jumps and never waits.
        for pair in starts.windows(2).skip(2) {
            assert!((pair[1] - pair[0] - HORIZONTAL).abs() < 1e-9, "{pair:?}");
        }
        let lag = starts[11] - arrivals[11];
        assert!(lag.abs() <= CHAIN_LEAD_SECONDS);
        let steady: Vec<_> = samples
            .windows(2)
            .filter(|pair| pair[0].0 > starts[2] && pair[1].0 < starts[11] + HORIZONTAL)
            .map(|pair| pair[1].1 - pair[0].1)
            .collect();
        let speed = 0.001 / HORIZONTAL;
        assert!(steady.iter().all(|delta| (delta - speed).abs() < 1e-6));
    }

    #[test]
    fn a_step_long_after_the_last_one_starts_from_where_it_stands() {
        let mut track = Track::new((0.0, 0.0));
        track.slide((0.0, 1.0), VERTICAL, 0.0);
        let start = track.slide((0.0, 2.0), VERTICAL, 5.0);
        assert_eq!(start, 5.0);
        assert!(close(track.position(5.0), (0.0, 1.0)));
        // A step arriving mid-slide is not chained: it continues from where
        // the sprite is drawn, so nothing jumps.
        let drawn = track.position(5.1);
        track.slide((0.0, 3.0), VERTICAL, 5.1);
        assert!(close(track.position(5.1), drawn));
    }

    fn grid() -> Grid {
        Grid {
            columns: 40,
            rows: 20,
            cell_width: 10,
            cell_height: 20,
        }
    }

    fn sprite(id: &str, x: i32, y: i32, motion: Option<f64>) -> Value {
        let mut value = json!({"id":id,"asset":"hero.png","x":x,"y":y,"width":48,"height":48,
            "anchor":"bottom_center","layer":0,"order":0});
        if let Some(duration) = motion {
            value["motion"] = json!({"duration": duration});
        }
        json!({"op":"put","kind":"sprite","id":id,"value":value})
    }

    fn view(column: i32, row: i32, follow: bool) -> Value {
        let mut value = json!({"scale":1,"origin":{"x":0,"y":0},
            "worldOrigin":{"column":column,"row":row},
            "clipRect":{"x":0,"y":0,"width":400,"height":400},
            "worldId":"map","textLayerIds":["npc","field-prompt"],"spriteIds":["player","npc:guide"]});
        if follow {
            value["follow"] = json!({"spriteId":"player","textLayerIds":["field-prompt"]});
        }
        value
    }

    struct Field {
        session: RetainedSession,
        motion: FieldMotion,
        generation: u64,
        scene: Option<std::sync::Arc<SceneSource>>,
    }

    impl Field {
        fn new(operations: Vec<Value>, viewport: Value) -> Self {
            let mut field = Self {
                session: RetainedSession::default(),
                motion: FieldMotion::default(),
                generation: 0,
                scene: None,
            };
            let mut operations = operations;
            operations.insert(
                0,
                json!({"op":"put","kind":"world","id":"map","value":{
                    "columns":200,"rows":200,"cellWidth":24,"cellHeight":48,
                    "layers":[{"id":"map:terrain","layer":-100,"kind":"gameplay"}]}}),
            );
            operations.push(json!({"op":"worldRows","id":"map","rows":(0..200).map(|row| json!({"row":row,"cells":[]})).collect::<Vec<_>>()}));
            operations.push(json!({"op":"put","kind":"text","id":"npc","value":{"id":"npc","layer":100,"order":0,"runs":[]}}));
            operations.push(json!({"op":"put","kind":"text","id":"field-prompt","value":{"id":"field-prompt","layer":1100,"order":1,"runs":[]}}));
            field.present(operations, Some(viewport), 0.0);
            field
        }

        fn present(&mut self, operations: Vec<Value>, viewport: Option<Value>, now: f64) {
            let base = self.generation;
            self.generation += 1;
            let mut update = json!({"frame":self.generation,"baseGeneration":base,
                "generation":self.generation,"reset":base == 0,"operations":operations});
            if let Some(viewport) = viewport {
                update["viewport"] = viewport;
            }
            let update: FrameUpdate = serde_json::from_value(update).unwrap();
            let accepted = self.session.apply(update, grid()).unwrap();
            self.motion
                .observe(&accepted.scene, accepted.viewport.as_ref(), now);
            self.scene = Some(accepted.scene);
        }

        fn viewport(&self, column: i32, row: i32) -> Viewport {
            serde_json::from_value(view(column, row, true)).unwrap()
        }
    }

    #[test]
    fn the_camera_follows_a_sliding_player_on_one_clock() {
        // The player stands at camera-screen (10, 5) with the camera at
        // world (20, 30); a guide stands still at world (35, 40).
        let mut field = Field::new(
            vec![
                sprite("player", 10, 5, None),
                sprite("npc:guide", 15, 10, None),
            ],
            view(20, 30, true),
        );
        // One step down: PHP scrolls the camera, so the player keeps its
        // screen cell and the guide's screen cell moves up.
        field.present(
            vec![
                sprite("player", 10, 5, Some(VERTICAL)),
                sprite("npc:guide", 15, 9, None),
            ],
            Some(view(20, 31, true)),
            1.0,
        );
        let viewport = field.viewport(20, 31);
        for step in 0..=16 {
            let now = 1.0 + VERTICAL * f64::from(step) / 16.0;
            let progress = f64::from(step) / 16.0;
            // The followed player never moves on screen, nor does its prompt.
            let player = field.motion.get_sprite_shift("player", now);
            assert!(player.0.abs() < 1e-5 && player.1.abs() < 1e-5, "{player:?}");
            let prompt = field.motion.get_text_shift(&viewport, "field-prompt", now);
            assert!(prompt.1.abs() < 1e-5);
            // The field, the guide and glyph text slide up under it together.
            let camera = field.motion.get_camera_shift(now);
            assert!((f64::from(camera.1) - (1.0 - progress)).abs() < 1e-5);
            assert_eq!(field.motion.get_sprite_shift("npc:guide", now), camera);
            assert_eq!(field.motion.get_text_shift(&viewport, "npc", now), camera);
            let world = field
                .motion
                .get_world_viewport(&viewport, (24.0, 48.0), now);
            let drawn = 30.0 + progress;
            assert_eq!(world.world_origin.row, drawn.floor() as i32);
            assert!(
                (f64::from(world.origin.y) + (drawn - drawn.floor()) * 48.0).abs() < 1e-3,
                "{}",
                world.origin.y
            );
        }
        assert!(field.motion.is_animating(1.0 + VERTICAL / 2.0));
        assert!(!field.motion.is_animating(1.0 + VERTICAL));
    }

    #[test]
    fn a_camera_that_does_not_scroll_lets_the_player_slide_across_the_screen() {
        let mut field = Field::new(
            vec![
                sprite("player", 1, 5, None),
                sprite("npc:guide", 3, 3, None),
            ],
            view(0, 30, true),
        );
        field.present(
            vec![sprite("player", 0, 5, Some(HORIZONTAL))],
            Some(view(0, 30, true)),
            2.0,
        );
        let viewport = field.viewport(0, 30);
        let halfway = 2.0 + HORIZONTAL / 2.0;
        assert_eq!(field.motion.get_camera_shift(halfway), (0.0, 0.0));
        assert_eq!(field.motion.get_sprite_shift("player", halfway), (0.5, 0.0));
        // The prompt drawn beside the player moves with it.
        assert_eq!(
            field
                .motion
                .get_text_shift(&viewport, "field-prompt", halfway),
            (0.5, 0.0)
        );
        assert_eq!(
            field.motion.get_text_shift(&viewport, "npc", halfway),
            (0.0, 0.0)
        );
    }

    #[test]
    fn placements_and_unhinted_changes_snap() {
        let mut field = Field::new(
            vec![
                sprite("player", 10, 5, None),
                sprite("npc:guide", 3, 3, None),
            ],
            view(20, 30, true),
        );
        // A teleport several cells away, even with a hint, is a placement.
        field.present(vec![sprite("player", 14, 5, Some(VERTICAL))], None, 1.0);
        assert_eq!(field.motion.get_sprite_shift("player", 1.0), (0.0, 0.0));
        // A step without a hint (a renderer-independent placement) snaps too.
        field.present(vec![sprite("player", 14, 6, None)], None, 2.0);
        assert!(!field.motion.is_animating(2.0));
        // A camera jump without a followed slide snaps: a cinematic pan.
        field.present(
            vec![sprite("player", 14, 5, None)],
            Some(view(20, 31, true)),
            3.0,
        );
        assert_eq!(field.motion.get_camera_shift(3.0), (0.0, 0.0));
        // Without follow the camera snaps even while the player slides.
        field.present(
            vec![sprite("player", 14, 5, Some(VERTICAL))],
            Some(view(20, 32, false)),
            4.0,
        );
        assert_eq!(field.motion.get_camera_shift(4.0), (0.0, 0.0));
        assert_eq!(field.motion.get_sprite_shift("player", 4.0), (0.0, -1.0));
        // A removed sprite forgets its slide; one put again appears in place.
        field.present(
            vec![json!({"op":"remove","kind":"sprite","id":"player"})],
            Some(
                json!({"scale":1,"origin":{"x":0,"y":0},"worldOrigin":{"column":20,"row":32},
                "clipRect":{"x":0,"y":0,"width":400,"height":400},"worldId":"map",
                "textLayerIds":[],"spriteIds":[]}),
            ),
            4.05,
        );
        field.present(
            vec![sprite("player", 14, 6, Some(VERTICAL))],
            Some(view(20, 32, true)),
            4.1,
        );
        assert!(!field.motion.is_animating(4.1));
        // A different world starts over.
        field.present(vec![], Some(json!(null)), 5.0);
        assert!(field.motion.get_camera_origin(5.0).is_none());
    }

    fn lifted(id: &str, x: i32, y: i32, motion: Option<f64>, lift: u32) -> Value {
        let mut put = sprite(id, x, y, motion);
        put["value"]["lift"] = json!(lift);
        put
    }

    #[test]
    fn a_lifted_sprite_slides_and_is_followed_as_its_cell_is_drawn_a_lift_higher() {
        use crate::renderer::{content_geometry_retained, sprite_bounds};
        use crate::viewport::ViewportTransform;
        let cell = (24.0, 48.0);
        let mut field = Field::new(
            vec![
                lifted("player", 10, 5, None, 6),
                lifted("npc:guide", 15, 10, None, 6),
            ],
            view(20, 30, true),
        );
        // The camera follows the player one step down while the guide walks
        // one step across.
        field.present(
            vec![
                lifted("player", 10, 5, Some(VERTICAL), 6),
                lifted("npc:guide", 16, 9, Some(HORIZONTAL), 6),
            ],
            Some(view(20, 31, true)),
            1.0,
        );
        let scene = field.scene.clone().unwrap();
        let viewport = field.viewport(20, 31);
        for scale in [1.0, 0.5] {
            let base = ViewportTransform::fit(400.0, 400.0, 400.0 * scale, 400.0 * scale);
            let (resting, _) = content_geometry_retained(base, &viewport);
            for step in 0..=16 {
                let now = 1.0 + VERTICAL * f64::from(step) / 16.0;
                for id in ["player", "npc:guide"] {
                    let item = &scene.sprites[id].item;
                    let shift = field.motion.get_sprite_shift(id, now);
                    let shifted = shift_viewport(&viewport, shift, cell);
                    let (content, _) = content_geometry_retained(base, &shifted);
                    let placed = sprite_bounds(item, 0, cell, content);
                    let drawn = sprite_bounds(item, scene.get_sprite_lift(id), cell, content);
                    // The sprite slides exactly as its cell does, and the lift
                    // only raises where that slide is drawn.
                    let rest = sprite_bounds(item, 0, cell, resting);
                    assert!((placed.left - rest.left - shift.0 * cell.0 * scale).abs() < 1e-3);
                    assert!((placed.top - rest.top - shift.1 * cell.1 * scale).abs() < 1e-3);
                    assert_eq!(drawn.left, placed.left);
                    assert!((placed.top - drawn.top - 6.0 * scale).abs() < 1e-3, "{id}");
                }
                // The followed player holds still on screen throughout.
                let player = field.motion.get_sprite_shift("player", now);
                assert!(player.0.abs() < 1e-5 && player.1.abs() < 1e-5);
            }
        }
    }

    #[test]
    fn motion_hints_and_follow_are_validated() {
        let mut session = RetainedSession::default();
        let invalid = [
            json!({"duration":0}),
            json!({"duration":-1}),
            json!({"duration":61}),
            json!({"duration":0.2,"from":1}),
        ];
        for motion in invalid {
            let mut put = sprite("player", 0, 0, None);
            put["value"]["motion"] = motion;
            let update: FrameUpdate = serde_json::from_value(json!({"frame":1,"baseGeneration":0,
                "generation":1,"reset":true,"present":false,"operations":[put]}))
            .unwrap();
            assert!(session.apply(update, grid()).is_err());
        }
        let mut field = Field::new(
            vec![
                sprite("player", 0, 0, None),
                sprite("npc:guide", 3, 3, None),
            ],
            view(0, 0, true),
        );
        let mut unknown = view(0, 0, true);
        unknown["follow"]["spriteId"] = json!("nobody");
        let base = field.generation;
        let update: FrameUpdate = serde_json::from_value(json!({"frame":9,"baseGeneration":base,
            "generation":base + 1,"operations":[],"viewport":unknown}))
        .unwrap();
        assert!(field.session.apply(update, grid()).is_err());
    }
}
