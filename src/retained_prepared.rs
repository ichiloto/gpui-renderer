//! Prepared resources for a retained scene. A camera-only frame shares this
//! entire scene; screen art is decoded only on source changes.
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
}

#[derive(Debug)]
pub struct PreparedWorld {
    pub source: Arc<World>,
    /// Layers in paint order.
    pub layers: Vec<PreparedWorldLayer>,
}

impl PreparedWorld {
    fn prepare(source: Arc<World>) -> Self {
        let mut layers: Vec<_> = source
            .definition
            .layers
            .iter()
            .map(|layer| PreparedWorldLayer {
                id: layer.id.clone(),
                layer: layer.layer,
            })
            .collect();
        layers.sort_by_key(|layer| layer.layer);
        Self { source, layers }
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
                _ => Arc::new(PreparedWorld::prepare(world.clone())),
            };
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
