//! Transactional retained source state. The reader validates a candidate before
//! replacing the accepted staging generation; a rejected update never alters
//! the visible presentation.
use crate::{
    canvas_protocol::{Canvas, CanvasImage, CanvasText, Indicator},
    composite_protocol::Composite,
    protocol::{FrameV2, Grid, Sprite, TextLayer},
    retained_protocol::{
        CanvasRoot, EntityKind, FrameUpdate, MAX_RETAINED_BYTES, MAX_STAGING_AND_VISIBLE_BYTES,
        Operation, ScreenText, SpriteMotion, SpritePivot, Viewport, ViewportChange,
        parse_sprite_quarter_turns, validate_id, validate_sprite_lift,
    },
    retained_world::World,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

#[derive(Clone, Debug)]
pub struct Ordered<T> {
    pub order: u32,
    pub item: T,
    pub wire_bytes: usize,
}

#[derive(Clone, Debug, Default)]
pub struct SceneSource {
    pub worlds: BTreeMap<String, Arc<World>>,
    pub text: BTreeMap<String, Arc<ScreenText>>,
    pub sprites: BTreeMap<String, Arc<Ordered<Sprite>>>,
    /// `field_motion` hints of the sprites that carry one, by sprite id.
    pub sprite_motions: BTreeMap<String, SpriteMotion>,
    /// `sprite_lift` lifts of the sprites that carry a nonzero one, by sprite id.
    pub sprite_lifts: BTreeMap<String, u32>,
    /// Explicit retained sprite rotations, including zero for capability gating.
    pub sprite_quarter_turns: BTreeMap<String, u8>,
    /// Explicit normalized pivots; omission preserves legacy bottom-center placement.
    pub sprite_pivots: BTreeMap<String, SpritePivot>,
    pub canvas: Option<CanvasRoot>,
    pub canvas_images: BTreeMap<String, Arc<Ordered<CanvasImage>>>,
    pub canvas_indicators: BTreeMap<String, Arc<Ordered<Indicator>>>,
    pub canvas_text: BTreeMap<String, Arc<Ordered<CanvasText>>>,
    pub canvas_composites: BTreeMap<String, Arc<Ordered<Composite>>>,
}

impl SceneSource {
    pub fn apply(&mut self, operation: Operation, grid: Grid) -> Result<(), String> {
        match operation {
            Operation::Put { kind, id, value } => {
                validate_id(&id)?;
                match kind {
                    EntityKind::World => {
                        let definition = parse_value(value)?;
                        self.worlds.insert(id, Arc::new(World::new(definition)?));
                    }
                    EntityKind::Text => {
                        let text: ScreenText = parse_value(value)?;
                        if text.id != id {
                            return Err("text put id differs from value id".into());
                        }
                        self.text.insert(id, Arc::new(text));
                    }
                    EntityKind::Sprite => {
                        // Retained drawing options live beside the shared
                        // sprite geometry used by every protocol version.
                        let mut value = object_value(value)?;
                        let motion = value
                            .remove("motion")
                            .map(|motion| parse_value::<SpriteMotion>(motion)?.validate())
                            .transpose()?;
                        let lift = value.remove("lift").map(parse_value::<u32>).transpose()?;
                        let pivot = value
                            .remove("pivot")
                            .map(|pivot| parse_value::<SpritePivot>(pivot)?.validate())
                            .transpose()?;
                        let quarter_turns = value
                            .remove("quarterTurns")
                            .map(|turns| parse_sprite_quarter_turns(&turns))
                            .transpose()?;
                        let sprite: Ordered<Sprite> =
                            parse_ordered(id.as_str(), Value::Object(value))?;
                        let lift = lift
                            .map(|lift| validate_sprite_lift(lift, sprite.item.height))
                            .transpose()?
                            .filter(|lift| *lift > 0);
                        self.sprites.insert(id.clone(), Arc::new(sprite));
                        match motion {
                            Some(motion) => self.sprite_motions.insert(id.clone(), motion),
                            None => self.sprite_motions.remove(&id),
                        };
                        match lift {
                            Some(lift) => self.sprite_lifts.insert(id.clone(), lift),
                            None => self.sprite_lifts.remove(&id),
                        };
                        match quarter_turns {
                            Some(turns) => self.sprite_quarter_turns.insert(id.clone(), turns),
                            None => self.sprite_quarter_turns.remove(&id),
                        };
                        match pivot {
                            Some(pivot) => self.sprite_pivots.insert(id, pivot),
                            None => self.sprite_pivots.remove(&id),
                        };
                    }
                    EntityKind::Canvas => {
                        if id != "canvas" {
                            return Err("canvas root id must be canvas".into());
                        }
                        let mut value = object_value(value)?;
                        value.remove("id");
                        value.remove("order");
                        self.canvas = Some(parse_value(Value::Object(value))?);
                    }
                    EntityKind::CanvasImage => {
                        self.canvas_images
                            .insert(id.clone(), Arc::new(parse_ordered(&id, value)?));
                    }
                    EntityKind::CanvasIndicator => {
                        self.canvas_indicators
                            .insert(id.clone(), Arc::new(parse_ordered(&id, value)?));
                    }
                    EntityKind::CanvasText => {
                        self.canvas_text
                            .insert(id.clone(), Arc::new(parse_ordered(&id, value)?));
                    }
                    EntityKind::CanvasComposite => {
                        self.canvas_composites
                            .insert(id.clone(), Arc::new(parse_ordered(&id, value)?));
                    }
                }
            }
            Operation::Remove { kind, id } => {
                validate_id(&id)?;
                match kind {
                    EntityKind::World => {
                        self.worlds.remove(&id);
                    }
                    EntityKind::Text => {
                        self.text.remove(&id);
                    }
                    EntityKind::Sprite => {
                        self.sprites.remove(&id);
                        self.sprite_motions.remove(&id);
                        self.sprite_lifts.remove(&id);
                        self.sprite_quarter_turns.remove(&id);
                        self.sprite_pivots.remove(&id);
                    }
                    EntityKind::Canvas => {
                        if id != "canvas" {
                            return Err("canvas root id must be canvas".into());
                        }
                        self.canvas = None;
                    }
                    EntityKind::CanvasImage => {
                        self.canvas_images.remove(&id);
                    }
                    EntityKind::CanvasIndicator => {
                        self.canvas_indicators.remove(&id);
                    }
                    EntityKind::CanvasText => {
                        self.canvas_text.remove(&id);
                    }
                    EntityKind::CanvasComposite => {
                        self.canvas_composites.remove(&id);
                    }
                }
            }
            Operation::WorldRows { id, rows } => {
                let world = self
                    .worlds
                    .get_mut(&id)
                    .ok_or("worldRows references an unknown world")?;
                Arc::make_mut(world).replace_rows(rows)?;
            }
            Operation::WorldTiles { id, layer_id, rows } => {
                let world = self
                    .worlds
                    .get_mut(&id)
                    .ok_or("worldTiles references an unknown world")?;
                Arc::make_mut(world).replace_tile_rows(&layer_id, rows)?;
            }
            Operation::TextRows { id, rows } => {
                let text = self
                    .text
                    .get_mut(&id)
                    .ok_or("textRows references an unknown text layer")?;
                let text = Arc::make_mut(text);
                let mut seen = HashSet::new();
                for row in &rows {
                    if row.row >= grid.rows
                        || !seen.insert(row.row)
                        || row.runs.iter().any(|run| run.row != row.row)
                    {
                        return Err("textRows contains an invalid or repeated row".into());
                    }
                }
                let replaced: HashSet<_> = rows.iter().map(|row| row.row).collect();
                text.runs.retain(|run| !replaced.contains(&run.row));
                for row in rows {
                    text.runs.extend(row.runs);
                }
                text.runs.sort_by_key(|run| run.row);
            }
        }
        if self.estimated_bytes() > MAX_RETAINED_BYTES {
            return Err("retained scene exceeds 64 MiB source budget".into());
        }
        Ok(())
    }

    pub fn validate_visible(&self, grid: Grid, viewport: Option<&Viewport>) -> Result<(), String> {
        let canvas_overlay = self.canvas.as_ref().is_some_and(CanvasRoot::is_overlay);
        if let Some(root) = &self.canvas
            && root.is_overlay()
            && root.background.is_some()
        {
            return Err("overlay canvas root cannot have a background".into());
        }
        if let Some(viewport) = viewport {
            viewport.validate(grid)?;
            if let Some(world_id) = &viewport.world_id {
                let world = self
                    .worlds
                    .get(world_id)
                    .ok_or("viewport references an unknown world")?;
                world.validate_complete()?;
            }
            for id in &viewport.text_layer_ids {
                if !self.text.contains_key(id) {
                    return Err("viewport references unknown text layer".into());
                }
            }
            for id in &viewport.sprite_ids {
                if !self.sprites.contains_key(id) {
                    return Err("viewport references unknown sprite".into());
                }
            }
            if self.canvas.is_some() && !canvas_overlay {
                return Err("world viewport and canvas cannot both be visible".into());
            }
        }
        self.materialize_screen(0)?
            .validate_retained(grid, canvas_overlay)
    }

    pub fn materialize_canvas(&self) -> Result<Option<Canvas>, String> {
        let Some(root) = &self.canvas else {
            if !self.canvas_images.is_empty()
                || !self.canvas_indicators.is_empty()
                || !self.canvas_text.is_empty()
                || !self.canvas_composites.is_empty()
            {
                return Err("canvas elements require a canvas root".into());
            }
            return Ok(None);
        };
        Ok(Some(Canvas {
            width: root.width,
            height: root.height,
            images: ordered_values(&self.canvas_images),
            indicators: ordered_values(&self.canvas_indicators),
            text_layers: ordered_values(&self.canvas_text),
            composites: Some(ordered_values(&self.canvas_composites)),
        }))
    }

    /// Logical pixels this sprite is drawn above its cell; 0 without a lift.
    pub fn get_sprite_lift(&self, id: &str) -> u32 {
        self.sprite_lifts.get(id).copied().unwrap_or_default()
    }

    pub fn get_sprite_pivot(&self, id: &str) -> Option<SpritePivot> {
        self.sprite_pivots.get(id).copied()
    }

    /// Clockwise screen-space quarter turns; omission leaves the image unrotated.
    pub fn get_sprite_quarter_turns(&self, id: &str) -> u8 {
        self.sprite_quarter_turns
            .get(id)
            .copied()
            .unwrap_or_default()
    }

    pub fn materialize_screen(&self, frame: u64) -> Result<FrameV2, String> {
        let mut text: Vec<_> = self.text.values().collect();
        text.sort_by_key(|item| item.order);
        let mut sprites: Vec<_> = self.sprites.values().collect();
        sprites.sort_by_key(|item| item.order);
        Ok(FrameV2 {
            frame,
            text_layers: text
                .into_iter()
                .map(|item| TextLayer {
                    id: item.id.clone(),
                    layer: item.layer,
                    runs: item.runs.clone(),
                })
                .collect(),
            sprites: sprites.into_iter().map(|item| item.item.clone()).collect(),
            tile_batches: None,
            canvas: self.materialize_canvas()?,
            viewport: None,
        })
    }

    pub fn estimated_bytes(&self) -> usize {
        let world: usize = self
            .worlds
            .values()
            .map(|world| world.estimated_bytes())
            .sum();
        let text: usize = self
            .text
            .values()
            .map(|item| {
                item.id.len()
                    + 128
                    + item
                        .runs
                        .iter()
                        .map(|run| run.text.len() + 80)
                        .sum::<usize>()
            })
            .sum();
        let sprite: usize = self
            .sprites
            .values()
            .map(|item| item.wire_bytes + 128)
            .sum();
        let canvas: usize = self
            .canvas_images
            .values()
            .map(|item| item.wire_bytes)
            .sum::<usize>()
            + self
                .canvas_indicators
                .values()
                .map(|item| item.wire_bytes)
                .sum::<usize>()
            + self
                .canvas_text
                .values()
                .map(|item| item.wire_bytes)
                .sum::<usize>()
            + self
                .canvas_composites
                .values()
                .map(|item| item.wire_bytes)
                .sum::<usize>();
        world + text + sprite + canvas
    }
}

fn object_value(value: Value) -> Result<serde_json::Map<String, Value>, String> {
    value
        .as_object()
        .cloned()
        .ok_or("retained put value must be an object".into())
}

fn parse_value<T: DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| format!("invalid retained put value: {error}"))
}

fn parse_ordered<T: DeserializeOwned>(id: &str, value: Value) -> Result<Ordered<T>, String> {
    let wire_bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?.len();
    let mut value = object_value(value)?;
    if value.get("id").and_then(Value::as_str) != Some(id) {
        return Err("retained put id differs from value id".into());
    }
    let order = value
        .remove("order")
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok())
        .ok_or("retained put requires order")?;
    Ok(Ordered {
        order,
        item: parse_value(Value::Object(value))?,
        wire_bytes,
    })
}

fn ordered_values<T: Clone>(items: &BTreeMap<String, Arc<Ordered<T>>>) -> Vec<T> {
    let mut values: Vec<_> = items.values().collect();
    values.sort_by_key(|item| item.order);
    values.into_iter().map(|item| item.item.clone()).collect()
}

#[derive(Debug)]
pub struct Accepted {
    pub frame: u64,
    pub reset: bool,
    pub visible: bool,
    pub scene: Arc<SceneSource>,
    pub viewport: Option<Viewport>,
}

#[derive(Clone, Default)]
pub struct RetainedSession {
    generation: u64,
    staging: Arc<SceneSource>,
    visible: Arc<SceneSource>,
    viewport: Option<Viewport>,
    needs_reset: bool,
}

impl RetainedSession {
    pub fn expected_generation(&self) -> u64 {
        self.generation
    }
    pub fn require_reset(&mut self) {
        self.needs_reset = true;
    }

    pub fn apply(&mut self, update: FrameUpdate, grid: Grid) -> Result<Accepted, String> {
        update.validate_envelope()?;
        if update.generation <= self.generation {
            return Err("stale retained generation".into());
        }
        if self.needs_reset && !update.reset {
            return Err("retained state requires a reset".into());
        }
        if !update.reset && update.base_generation != self.generation {
            self.needs_reset = true;
            return Err("retained baseGeneration differs from the accepted generation".into());
        }
        // Reset intentionally tolerates a mismatched base after a lost or
        // rejected update, but its generation must still advance monotonically.
        let candidate = if update.reset || !update.operations.is_empty() {
            let mut candidate = if update.reset {
                SceneSource::default()
            } else {
                (*self.staging).clone()
            };
            for operation in update.operations {
                candidate.apply(operation, grid)?;
            }
            Arc::new(candidate)
        } else {
            self.staging.clone()
        };
        let next_viewport = match update.viewport {
            ViewportChange::Keep => self.viewport.clone(),
            ViewportChange::Clear => None,
            ViewportChange::Set(viewport) => Some(viewport),
        };
        if update.present {
            candidate.validate_visible(grid, next_viewport.as_ref())?;
        }
        if candidate.estimated_bytes() + self.visible.estimated_bytes()
            > MAX_STAGING_AND_VISIBLE_BYTES
        {
            return Err("retained staging and visible scenes exceed 128 MiB".into());
        }
        self.generation = update.generation;
        self.staging = candidate.clone();
        self.needs_reset = false;
        if update.present {
            self.visible = candidate.clone();
            self.viewport = next_viewport.clone();
        }
        Ok(Accepted {
            frame: update.frame,
            reset: update.reset,
            visible: update.present,
            scene: candidate,
            viewport: next_viewport,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_value, json};

    fn grid() -> Grid {
        Grid {
            columns: 4,
            rows: 2,
            cell_width: 10,
            cell_height: 20,
        }
    }

    fn row(row: u32, widths: &[u8]) -> Value {
        json!({"row":row,"cells":widths.iter().map(|width| json!({
            "glyph":if *width == 2 {"界"} else {".."},
            "foreground":null,"background":null,
            "ownerLayerId":"map:terrain"
        })).collect::<Vec<_>>()})
    }

    fn update(
        base: u64,
        generation: u64,
        reset: bool,
        present: bool,
        operations: Vec<Value>,
        viewport: Option<Value>,
    ) -> FrameUpdate {
        let mut value = json!({"frame":1,"baseGeneration":base,"generation":generation,
            "reset":reset,"present":present,"operations":operations});
        if let Some(viewport) = viewport {
            value["viewport"] = viewport;
        }
        from_value(value).unwrap()
    }

    fn world() -> Value {
        json!({"op":"put","kind":"world","id":"map","value":{
            "columns":4,"rows":2,"cellWidth":5,"cellHeight":10,"layers":[{"id":"map:terrain","layer":-100,
                "kind":"gameplay"}]}})
    }

    fn viewport() -> Value {
        json!({"scale":1,"origin":{"x":0,"y":0},
            "worldOrigin":{"column":0,"row":0},
            "clipRect":{"x":0,"y":0,"width":40,"height":40},
            "worldId":"map","textLayerIds":[],"spriteIds":[]})
    }

    fn overlay_root() -> Value {
        json!({"op":"put","kind":"canvas","id":"canvas","value":{
            "width":40,"height":40,"mode":"overlay"}})
    }

    fn overlay_image(x: u32) -> Value {
        json!({"op":"put","kind":"canvas_image","id":"effect","value":{
            "id":"effect","order":0,"asset":"effect.png","layer":500,
            "destination":{"x":x,"y":0,"width":10,"height":10}}})
    }

    #[test]
    fn overlay_only_updates_and_close_preserve_field_and_screen_identity() {
        let mut session = RetainedSession::default();
        let first = session
            .apply(
                update(
                    0,
                    1,
                    true,
                    true,
                    vec![
                        world(),
                        json!({"op":"worldRows","id":"map","rows":[
                            row(0,&[1,1,1,1]),row(1,&[1,1,1,1])]}),
                        json!({"op":"put","kind":"text","id":"hud","value":{
                            "id":"hud","layer":1000,"order":0,"runs":[{
                                "row":0,"column":0,"text":"HP","foreground":null,"background":null}]}}),
                        json!({"op":"put","kind":"sprite","id":"hero","value":{
                            "id":"hero","order":0,"asset":"hero.png","x":10,"y":20,
                            "width":10,"height":20,"anchor":"bottom_center","layer":100}}),
                        overlay_root(),
                        overlay_image(0),
                    ],
                    Some(viewport()),
                ),
                grid(),
            )
            .unwrap();
        assert!(first.scene.canvas.as_ref().unwrap().is_overlay());
        assert!(first.viewport.is_some());
        let screen = first.scene.materialize_screen(1).unwrap();
        assert_eq!(screen.text_layers[0].id, "hud");
        assert_eq!(screen.sprites[0].id, "hero");
        assert_eq!(screen.canvas.as_ref().unwrap().images[0].id, "effect");

        let second = session
            .apply(
                update(1, 2, false, true, vec![overlay_image(10)], None),
                grid(),
            )
            .unwrap();
        assert!(Arc::ptr_eq(
            &first.scene.worlds["map"],
            &second.scene.worlds["map"]
        ));
        assert!(Arc::ptr_eq(
            &first.scene.text["hud"],
            &second.scene.text["hud"]
        ));
        assert!(Arc::ptr_eq(
            &first.scene.sprites["hero"],
            &second.scene.sprites["hero"]
        ));
        assert_eq!(
            second
                .scene
                .materialize_screen(2)
                .unwrap()
                .canvas
                .unwrap()
                .images[0]
                .destination
                .x,
            10.0
        );
        assert!(second.viewport.is_some());

        let invalid = session.apply(
            update(2, 3, false, true, vec![overlay_image(31)], None),
            grid(),
        );
        assert_eq!(
            invalid.unwrap_err(),
            "canvas rectangle must be finite, positive and within the canvas"
        );
        assert_eq!(session.expected_generation(), 2);
        assert!(Arc::ptr_eq(&session.visible, &second.scene));

        let closed = session
            .apply(
                update(
                    2,
                    3,
                    false,
                    true,
                    vec![
                        json!({"op":"remove","kind":"canvas_image","id":"effect"}),
                        json!({"op":"remove","kind":"canvas","id":"canvas"}),
                    ],
                    None,
                ),
                grid(),
            )
            .unwrap();
        assert!(closed.scene.canvas.is_none());
        assert!(closed.scene.materialize_screen(3).unwrap().canvas.is_none());
        assert!(Arc::ptr_eq(
            &first.scene.worlds["map"],
            &closed.scene.worlds["map"]
        ));
        assert!(Arc::ptr_eq(
            &first.scene.text["hud"],
            &closed.scene.text["hud"]
        ));
        assert!(Arc::ptr_eq(
            &first.scene.sprites["hero"],
            &closed.scene.sprites["hero"]
        ));
        assert!(closed.viewport.is_some());
    }

    #[test]
    fn overlay_mode_rejects_invalid_root_and_element_geometry() {
        let absent_mode: CanvasRoot = serde_json::from_value(json!({
            "width":40,"height":40
        }))
        .unwrap();
        assert!(!absent_mode.is_overlay());
        let mut exclusive = SceneSource::default();
        exclusive
            .apply(
                from_value(json!({"op":"put","kind":"canvas","id":"canvas",
            "value":{"width":40,"height":40}}))
                .unwrap(),
                grid(),
            )
            .unwrap();
        exclusive
            .apply(
                from_value(json!({"op":"put","kind":"text","id":"hud",
            "value":{"id":"hud","layer":1000,"order":0,"runs":[]}}))
                .unwrap(),
                grid(),
            )
            .unwrap();
        assert_eq!(
            exclusive.validate_visible(grid(), None).unwrap_err(),
            "canvas cannot mix with text, sprites, tiles or viewport"
        );
        for mode in [json!("unknown"), json!(null), json!(1)] {
            assert!(
                serde_json::from_value::<CanvasRoot>(json!({
                    "width":40,"height":40,"mode":mode
                }))
                .is_err()
            );
        }

        let mut scene = SceneSource::default();
        scene
            .apply(from_value(overlay_root()).unwrap(), grid())
            .unwrap();
        assert!(scene.validate_visible(grid(), None).is_ok());
        for root in [
            json!({"width":39,"height":40,"mode":"overlay"}),
            json!({"width":40,"height":41,"mode":"overlay"}),
            json!({"width":40,"height":40,"mode":"overlay",
                "background":{"kind":"rgb","r":0,"g":0,"b":0}}),
        ] {
            let mut invalid = scene.clone();
            invalid
                .apply(
                    from_value(json!({"op":"put","kind":"canvas",
                "id":"canvas","value":root}))
                    .unwrap(),
                    grid(),
                )
                .unwrap();
            assert!(invalid.validate_visible(grid(), None).is_err(), "{root}");
        }
        let mut invalid = scene;
        invalid
            .apply(from_value(overlay_image(31)).unwrap(), grid())
            .unwrap();
        assert_eq!(
            invalid.validate_visible(grid(), None).unwrap_err(),
            "canvas rectangle must be finite, positive and within the canvas"
        );
    }

    #[test]
    fn staged_world_commits_once_and_scroll_reuses_the_scene() {
        let mut session = RetainedSession::default();
        let staged = session
            .apply(update(0, 1, true, false, vec![world()], None), grid())
            .unwrap();
        assert!(!staged.visible);
        assert_eq!(session.expected_generation(), 1);
        let staged = session
            .apply(
                update(
                    1,
                    2,
                    false,
                    false,
                    vec![json!({
                        "op":"worldRows","id":"map","rows":[row(0,&[1,2,1,1]),row(1,&[1,1,1,1])]
                    })],
                    None,
                ),
                grid(),
            )
            .unwrap();
        assert!(!staged.visible);
        let visible = session
            .apply(update(2, 3, false, true, vec![], Some(viewport())), grid())
            .unwrap();
        assert!(visible.visible);
        let source = visible.scene.clone();
        let changed = session
            .apply(
                update(
                    3,
                    4,
                    false,
                    true,
                    vec![],
                    Some(json!({
            "scale":1,"origin":{"x":0,"y":0},
            "worldOrigin":{"column":1,"row":0},
            "clipRect":{"x":0,"y":0,"width":40,"height":40},
            "worldId":"map"})),
                ),
                grid(),
            )
            .unwrap();
        assert!(Arc::ptr_eq(&source, &changed.scene));
        let world = changed.scene.worlds.get("map").unwrap();
        let mut projected = Vec::new();
        world.project_visible(changed.viewport.as_ref().unwrap(), |cell, _| {
            projected.push(cell)
        });
        assert_eq!(projected[0].world_column, 1);
        // A two-column glyph is one cell like any other; it shifts nothing.
        assert_eq!(projected[1].screen_column, 1);
        assert_eq!(session.expected_generation(), 4);
    }

    #[test]
    fn rejected_generation_preserves_visible_state_and_reset_recovers() {
        let mut session = RetainedSession::default();
        let first = session
            .apply(
                update(
                    0,
                    1,
                    true,
                    true,
                    vec![
                        world(),
                        json!({
                            "op":"worldRows","id":"map","rows":[row(0,&[1,1,1,1]),row(1,&[1,1,1,1])]
                        }),
                    ],
                    Some(viewport()),
                ),
                grid(),
            )
            .unwrap();
        let prior = first.scene.clone();
        assert!(
            session
                .apply(update(0, 2, false, true, vec![], None), grid())
                .is_err()
        );
        assert_eq!(session.expected_generation(), 1);
        assert!(
            session
                .apply(update(1, 3, false, true, vec![], None), grid())
                .is_err()
        );
        let recovered = session
            .apply(
                update(
                    999,
                    4,
                    true,
                    true,
                    vec![
                        world(),
                        json!({
                            "op":"worldRows","id":"map","rows":[row(0,&[1,1,1,1]),row(1,&[1,1,1,1])]
                        }),
                    ],
                    Some(viewport()),
                ),
                grid(),
            )
            .unwrap();
        assert!(recovered.reset);
        assert!(!Arc::ptr_eq(&prior, &recovered.scene));
        assert_eq!(session.expected_generation(), 4);
    }

    #[test]
    fn invalid_world_chunk_is_atomic() {
        let mut scene = SceneSource::default();
        scene.apply(from_value(world()).unwrap(), grid()).unwrap();
        let old = scene.clone();
        let bad: Operation = from_value(json!({"op":"worldRows","id":"map","rows":[
            row(0,&[1,1,1,1]),row(1,&[1,1,1,1,1])]}))
        .unwrap();
        assert!(scene.apply(bad, grid()).is_err());
        // SceneSource is a candidate and is discarded on error by RetainedSession.
        assert!(old.worlds.get("map").unwrap().get_row(0).is_none());
    }

    fn lifted_sprite(id: &str, order: u32, lift: Option<Value>) -> Operation {
        let mut value = json!({"id":id,"asset":"hero.png","x":1,"y":order,"width":48,
            "height":48,"anchor":"bottom_center","layer":100,"order":order});
        if let Some(lift) = lift {
            value["lift"] = lift;
        }
        from_value(json!({"op":"put","kind":"sprite","id":id,"value":value})).unwrap()
    }

    #[test]
    fn a_sprite_lift_is_validated_and_retained_beside_its_sprite() {
        let mut scene = SceneSource::default();
        scene
            .apply(lifted_sprite("behind", 0, Some(json!(48))), grid())
            .unwrap();
        scene
            .apply(lifted_sprite("front", 1, Some(json!(6))), grid())
            .unwrap();
        assert_eq!(
            (
                scene.get_sprite_lift("behind"),
                scene.get_sprite_lift("front")
            ),
            (48, 6)
        );
        // A lift is not sprite geometry: the sprite keeps its cell and its
        // place in the draw order, however high it is drawn.
        let screen = scene.materialize_screen(1).unwrap();
        assert_eq!(
            screen
                .sprites
                .iter()
                .map(|sprite| (sprite.id.as_str(), sprite.y))
                .collect::<Vec<_>>(),
            [("behind", 0), ("front", 1)]
        );
        // A put without a lift, or with none, draws the sprite on its cell again.
        scene
            .apply(lifted_sprite("front", 1, Some(json!(0))), grid())
            .unwrap();
        scene
            .apply(lifted_sprite("behind", 0, None), grid())
            .unwrap();
        assert!(scene.sprite_lifts.is_empty());
        scene
            .apply(lifted_sprite("front", 1, Some(json!(6))), grid())
            .unwrap();
        scene
            .apply(
                from_value(json!({"op":"remove","kind":"sprite","id":"front"})).unwrap(),
                grid(),
            )
            .unwrap();
        assert_eq!(scene.get_sprite_lift("front"), 0);
        for invalid in [
            json!(-6),
            json!(6.5),
            json!("6"),
            json!(null),
            json!(u64::MAX),
        ] {
            assert!(
                SceneSource::default()
                    .apply(lifted_sprite("hero", 0, Some(invalid.clone())), grid())
                    .is_err(),
                "{invalid}"
            );
        }
        assert_eq!(
            SceneSource::default()
                .apply(lifted_sprite("hero", 0, Some(json!(49))), grid())
                .unwrap_err(),
            "sprite lift must be at most the sprite's height"
        );
    }

    fn quarter_turned_sprite(id: &str, turns: Option<Value>) -> Operation {
        let mut value = json!({"id":id,"asset":"hero.png","x":7,"y":9,"width":48,
            "height":48,"anchor":"bottom_center","layer":100,"order":2,
            "sourceRect":{"x":3,"y":5,"width":12,"height":12}});
        if let Some(turns) = turns {
            value["quarterTurns"] = turns;
        }
        from_value(json!({"op":"put","kind":"sprite","id":id,"value":value})).unwrap()
    }

    fn pivoted_sprite(pivot: Option<Value>) -> Operation {
        let mut value = json!({"id":"effect","asset":"effect.png","x":7,"y":9,
            "width":144,"height":48,"anchor":"bottom_center","layer":100,"order":2,
            "sourceRect":{"x":12,"y":0,"width":12,"height":4}});
        if let Some(pivot) = pivot {
            value["pivot"] = pivot;
        }
        from_value(json!({"op":"put","kind":"sprite","id":"effect","value":value})).unwrap()
    }

    #[test]
    fn sprite_pivot_preserves_geometry_and_clears_on_replacement_removal_and_reset() {
        let mut scene = SceneSource::default();
        for (x, y) in [(0.0, 1.0), (1.0, 0.0), (0.25, 0.625)] {
            scene
                .apply(pivoted_sprite(Some(json!({"x":x,"y":y}))), grid())
                .unwrap();
            assert_eq!(scene.get_sprite_pivot("effect"), Some(SpritePivot { x, y }));
            let sprite = &scene.sprites["effect"].item;
            assert_eq!(
                (sprite.x, sprite.y, sprite.width, sprite.height),
                (7, 9, 144, 48)
            );
            assert_eq!(sprite.source_rect.unwrap().x, 12);
            assert_eq!(scene.materialize_screen(1).unwrap().sprites[0].y, 9);
        }
        scene.apply(pivoted_sprite(None), grid()).unwrap();
        assert_eq!(scene.get_sprite_pivot("effect"), None);
        scene
            .apply(pivoted_sprite(Some(json!({"x":0.25,"y":0.625}))), grid())
            .unwrap();
        scene
            .apply(
                from_value(json!({"op":"remove","kind":"sprite","id":"effect"})).unwrap(),
                grid(),
            )
            .unwrap();
        assert!(scene.sprite_pivots.is_empty());
        let mut session = RetainedSession::default();
        let mut first = update(0, 1, true, true, vec![], None);
        first.operations = vec![pivoted_sprite(Some(json!({"x":0.25,"y":0.625})))];
        session.apply(first, grid()).unwrap();
        let reset = session
            .apply(update(1, 2, true, true, vec![], None), grid())
            .unwrap();
        assert!(reset.scene.sprite_pivots.is_empty());
    }

    #[test]
    fn sprite_pivot_rejects_malformed_nonfinite_and_out_of_range_coordinates() {
        for value in [
            json!(null),
            json!(true),
            json!([]),
            json!({"x":0.5}),
            json!({"x":"0.5","y":1}),
            json!({"x":0.5,"y":1,"z":0}),
            json!({"x":-0.1,"y":1}),
            json!({"x":0.5,"y":1.1}),
        ] {
            assert!(
                SceneSource::default()
                    .apply(pivoted_sprite(Some(value.clone())), grid())
                    .is_err(),
                "{value}"
            );
        }
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(SpritePivot { x: bad, y: 0.5 }.validate().is_err());
            assert!(SpritePivot { x: 0.5, y: bad }.validate().is_err());
        }
    }

    #[test]
    fn sprite_quarter_turns_preserves_geometry_and_replacement_semantics() {
        let mut scene = SceneSource::default();
        assert_eq!(scene.get_sprite_quarter_turns("hero"), 0);
        for turns in 0..=3 {
            scene
                .apply(quarter_turned_sprite("hero", Some(json!(turns))), grid())
                .unwrap();
            assert_eq!(scene.get_sprite_quarter_turns("hero"), turns);
            assert_eq!(scene.sprite_quarter_turns.len(), 1);
            let sprite = &scene.sprites["hero"].item;
            assert_eq!(
                (sprite.x, sprite.y, sprite.width, sprite.height),
                (7, 9, 48, 48)
            );
            assert_eq!(sprite.source_rect.unwrap().x, 3);
        }
        scene
            .apply(quarter_turned_sprite("hero", None), grid())
            .unwrap();
        assert_eq!(scene.get_sprite_quarter_turns("hero"), 0);
        assert!(scene.sprite_quarter_turns.is_empty());
        scene
            .apply(quarter_turned_sprite("hero", Some(json!(2))), grid())
            .unwrap();
        scene
            .apply(
                from_value(json!({"op":"remove","kind":"sprite","id":"hero"})).unwrap(),
                grid(),
            )
            .unwrap();
        assert_eq!(scene.get_sprite_quarter_turns("hero"), 0);
        assert!(scene.sprite_quarter_turns.is_empty());
    }

    #[test]
    fn sprite_quarter_turns_accepts_only_integer_range_zero_to_three() {
        for invalid in [
            json!(-1),
            json!(4),
            json!(1.0),
            json!("1"),
            json!(null),
            json!(true),
            json!([]),
            json!(u64::MAX),
        ] {
            assert_eq!(
                SceneSource::default()
                    .apply(quarter_turned_sprite("hero", Some(invalid.clone())), grid())
                    .unwrap_err(),
                "sprite quarterTurns must be an integer from 0 to 3",
                "{invalid}"
            );
        }
    }
}
