//! Transactional retained source state. The reader validates a candidate before
//! replacing the accepted staging generation; a rejected update never alters
//! the visible presentation.
use crate::{
    canvas_protocol::{Canvas, CanvasImage, CanvasText, Indicator},
    composite_protocol::Composite,
    protocol::{FrameV2, Grid, Sprite, TextLayer},
    retained_protocol::{
        CanvasRoot, EntityKind, FrameUpdate, MAX_RETAINED_BYTES, MAX_STAGING_AND_VISIBLE_BYTES,
        Operation, ScreenText, SpriteMotion, Viewport, ViewportChange, validate_id,
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
                        // Motion is retained beside the shared sprite geometry,
                        // which every protocol version also uses.
                        let mut value = object_value(value)?;
                        let motion = value
                            .remove("motion")
                            .map(|motion| parse_value::<SpriteMotion>(motion)?.validate())
                            .transpose()?;
                        self.sprites.insert(
                            id.clone(),
                            Arc::new(parse_ordered(id.as_str(), Value::Object(value))?),
                        );
                        match motion {
                            Some(motion) => self.sprite_motions.insert(id, motion),
                            None => self.sprite_motions.remove(&id),
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
            if self.canvas.is_some() {
                return Err("world viewport and canvas cannot both be visible".into());
            }
        }
        let screen = FrameV2 {
            frame: 0,
            text_layers: self
                .text
                .values()
                .map(|text| TextLayer {
                    id: text.id.clone(),
                    layer: text.layer,
                    runs: text.runs.clone(),
                })
                .collect(),
            sprites: self
                .sprites
                .values()
                .map(|sprite| sprite.item.clone())
                .collect(),
            tile_batches: None,
            canvas: None,
            viewport: None,
        };
        screen.validate(grid)?;
        if let Some(canvas) = self.materialize_canvas()? {
            canvas.validate()?;
        }
        Ok(())
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
}
