//! Prepared resources for a retained scene. A camera-only frame shares this
//! entire scene; map atlases and screen art are decoded only on source changes.
use crate::{
    assets::AssetRoot, protocol::Grid, retained_state::SceneSource, retained_world::World,
    state::PreparedFrame,
};
use gpui::RenderImage;
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

#[derive(Debug)]
pub struct PreparedWorldLayer {
    pub id: String,
    pub layer: i32,
    pub regions: Vec<Arc<RenderImage>>,
}

#[derive(Debug)]
pub struct PreparedWorld {
    pub source: Arc<World>,
    pub layers: Vec<PreparedWorldLayer>,
    pub images: Vec<Arc<RenderImage>>,
}

impl PreparedWorld {
    fn prepare(source: Arc<World>, assets: &AssetRoot) -> Result<Self, String> {
        let mut layers = Vec::new();
        let mut images = Vec::new();
        let mut ids = HashSet::new();
        for layer in &source.definition.layers {
            let mut regions = Vec::new();
            if let Some(path) = &layer.asset {
                let prepared = (|| -> Result<_, String> {
                    let atlas = assets.load(path)?;
                    let size = atlas.size(0);
                    for rect in &layer.sources {
                        rect.validate_image(size.width.0 as u32, size.height.0 as u32)?;
                    }
                    let crops = layer
                        .sources
                        .iter()
                        .map(|rect| assets.prepare_region(&atlas, *rect))
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok((atlas, crops))
                })();
                match prepared {
                    Ok((atlas, crops)) => {
                        if ids.insert(atlas.id) {
                            images.push(atlas);
                        }
                        for image in &crops {
                            if ids.insert(image.id) {
                                images.push(image.clone());
                            }
                        }
                        regions = crops;
                    }
                    Err(error) => crate::protocol::diagnostic(format!(
                        "retained world layer {} cannot use atlas {}: {error}; painting glyph fallback",
                        layer.id,
                        path.display(),
                    )),
                }
            }
            layers.push(PreparedWorldLayer {
                id: layer.id.clone(),
                layer: layer.layer,
                regions,
            });
        }
        layers.sort_by_key(|layer| layer.layer);
        Ok(Self {
            source,
            layers,
            images,
        })
    }

    pub fn get_source(&self, layer: &PreparedWorldLayer, column: u32, row: u32) -> Option<usize> {
        let cells = &self.source.get_tile_row(&layer.id, row)?.cells;
        let index = cells
            .binary_search_by_key(&column, |cell| cell.column)
            .ok()?;
        let source = cells[index].source as usize;
        (source < layer.regions.len()).then_some(source)
    }
}

#[derive(Debug)]
pub struct PreparedScene {
    pub source: Arc<SceneSource>,
    pub screen: Arc<PreparedFrame>,
    pub worlds: BTreeMap<String, Arc<PreparedWorld>>,
    pub images: Vec<Arc<RenderImage>>,
}

impl PreparedScene {
    pub fn prepare(
        source: Arc<SceneSource>,
        frame: u64,
        grid: Grid,
        assets: &AssetRoot,
        previous: Option<&Self>,
    ) -> Result<Self, String> {
        let screen = Arc::new(PreparedFrame::prepare_v2(
            source.materialize_screen(frame)?,
            grid,
            assets,
        )?);
        let mut worlds = BTreeMap::new();
        let mut images = Vec::new();
        let mut ids = HashSet::new();
        for image in &screen.cached_images {
            if ids.insert(image.id) {
                images.push(image.clone());
            }
        }
        for (id, world) in &source.worlds {
            let prepared = match previous.and_then(|old| old.worlds.get(id)) {
                Some(old) if Arc::ptr_eq(world, &old.source) => old.clone(),
                _ => Arc::new(PreparedWorld::prepare(world.clone(), assets)?),
            };
            for image in &prepared.images {
                if ids.insert(image.id) {
                    images.push(image.clone());
                }
            }
            worlds.insert(id.clone(), prepared);
        }
        let bytes: usize = images
            .iter()
            .map(|image| image.as_bytes(0).map_or(0, |pixels| pixels.len()))
            .sum();
        // Source state has its own 64 MiB bound. Existing preparation separately
        // bounds decoded PNGs, guarded regions, and composite output at 64 MiB
        // each; their live images cannot share that single source-state budget.
        if bytes > 256 * 1024 * 1024 {
            return Err(format!(
                "retained prepared images exceed 256 MiB ({} bytes)",
                bytes
            ));
        }
        Ok(Self {
            source,
            screen,
            worlds,
            images,
        })
    }

    pub fn get_world(&self, id: &str) -> Option<&Arc<PreparedWorld>> {
        self.worlds.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_value, json};

    #[test]
    fn unavailable_atlases_keep_the_world_glyph_fallback() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let assets = AssetRoot::new(&root).unwrap();
        for (asset, x, available) in [
            ("test-sprite.png", 0, true),
            ("missing-tiles.png", 0, false),
            ("compositing/frame.json", 0, false),
            ("test-sprite.png", 9999, false),
        ] {
            let definition = from_value(json!({"columns":1,"rows":1,"cellSize":10,"layers":[{
                "id":"map:terrain","layer":-100,"kind":"gameplay",
                "asset":asset,"sources":[{"x":x,"y":0,"width":1,"height":1}]
            }]}))
            .unwrap();
            let mut world = World::new(definition).unwrap();
            world
                .replace_rows(vec![
                    from_value(json!({"row":0,"cells":[{
                        "glyph":".","foreground":null,"background":null,
                        "displayWidth":1,"ownerLayerId":"map:terrain"
                    }]}))
                    .unwrap(),
                ])
                .unwrap();
            world
                .replace_tile_rows(
                    "map:terrain",
                    vec![
                        from_value(json!({
                            "row":0,"cells":[{"column":0,"source":0}]
                        }))
                        .unwrap(),
                    ],
                )
                .unwrap();
            let prepared = PreparedWorld::prepare(Arc::new(world), &assets).unwrap();
            let source = prepared.get_source(&prepared.layers[0], 0, 0);
            assert_eq!(source, available.then_some(0), "asset {asset} at x={x}");
            assert_eq!(prepared.source.get_row(0).unwrap().cells[0].glyph, ".");
        }
    }
}
