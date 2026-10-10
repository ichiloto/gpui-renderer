//! Composed world tiles. Each available frame of each catalog tile is composed
//! once from its sheet pieces into its own guarded image, so display sampling
//! treats it like any prepared source region. Unusable art never rejects the
//! scene: the tiles that need it are unavailable and their cells keep glyphs.
use crate::{
    assets::AssetRoot,
    composite_pixels::{Pixel, blend, unpack, write},
    composite_protocol::Blend,
    retained_protocol::{TilePiece, Tileset},
    tile_regions::GUARD,
};
use gpui::RenderImage;
use std::sync::Arc;

/// Bound on the composed bytes of one tileset.
pub const MAX_COMPOSED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub struct PreparedTileset {
    /// Composed frames of each catalog tile; empty when it is unavailable.
    tiles: Vec<Vec<Arc<RenderImage>>>,
    /// Every composed image, for GPU retirement with the scene.
    pub images: Vec<Arc<RenderImage>>,
}

impl PreparedTileset {
    /// Loads the sheets and composes every available tile. The returned
    /// diagnostics hold at most one entry per sheet plus one for the budget.
    pub fn prepare(tileset: &Tileset, assets: &AssetRoot) -> (Self, Vec<String>) {
        let mut diagnostics = Vec::new();
        let sheets: Vec<Option<Sheet>> = tileset
            .sheets
            .iter()
            .map(|path| {
                match assets.load(path).and_then(|image| {
                    Sheet::new(image).ok_or_else(|| "decoded sheet has no pixels".to_string())
                }) {
                    Ok(sheet) => Some(sheet),
                    Err(error) => {
                        diagnostics.push(format!(
                            "tileset sheet {} is unavailable: {error}; its tiles show glyphs",
                            path.display()
                        ));
                        None
                    }
                }
            })
            .collect();
        let mut outside = vec![0usize; sheets.len()];
        let usable: Vec<bool> = tileset
            .tiles
            .iter()
            .map(|tile| {
                let mut usable = true;
                for piece in tile.frames.iter().flatten() {
                    let TilePiece::Sheet {
                        sheet,
                        x,
                        y,
                        width,
                        height,
                        ..
                    } = *piece
                    else {
                        continue;
                    };
                    match &sheets[sheet as usize] {
                        None => usable = false,
                        Some(source) if !source.contains_region(x, y, width, height) => {
                            outside[sheet as usize] += 1;
                            usable = false;
                        }
                        Some(_) => {}
                    }
                }
                usable
            })
            .collect();
        for (path, count) in tileset.sheets.iter().zip(&outside) {
            if *count > 0 {
                diagnostics.push(format!(
                    "tileset sheet {} is smaller than {count} of its pieces; tiles using them show glyphs",
                    path.display()
                ));
            }
        }
        let mut composed_bytes = 0;
        let mut tiles = Vec::with_capacity(tileset.tiles.len());
        let mut images = Vec::new();
        for (index, (tile, usable)) in tileset.tiles.iter().zip(usable).enumerate() {
            if !usable {
                tiles.push(Vec::new());
                continue;
            }
            let width = tile.get_width(tileset.tile_size);
            let bytes = ((width + 2 * GUARD) * (tileset.tile_size + 2 * GUARD) * 4) as usize
                * tile.frames.len();
            if composed_bytes + bytes > MAX_COMPOSED_BYTES {
                diagnostics.push(format!(
                    "tileset composition exceeds 64 MiB at tile {index}; later tiles show glyphs"
                ));
                break;
            }
            composed_bytes += bytes;
            let frames: Vec<_> = tile
                .frames
                .iter()
                .map(|pieces| compose(pieces, &sheets, width, tileset.tile_size))
                .collect();
            images.extend(frames.iter().cloned());
            tiles.push(frames);
        }
        tiles.resize_with(tileset.tiles.len(), Vec::new);
        (Self { tiles, images }, diagnostics)
    }

    pub fn is_available(&self, tile: u32) -> bool {
        self.tiles
            .get(tile as usize)
            .is_some_and(|frames| !frames.is_empty())
    }

    /// The composed frame a tile shows at this animation counter.
    pub fn get_frame(&self, tile: u32, tile_frame: u32) -> Option<&Arc<RenderImage>> {
        let frames = self.tiles.get(tile as usize)?;
        frames.get(select_frame(frames.len(), tile_frame)?)
    }
}

/// A tile with `count` frames shows frame `tile_frame % count`.
pub fn select_frame(count: usize, tile_frame: u32) -> Option<usize> {
    (count > 0).then(|| tile_frame as usize % count)
}

struct Sheet {
    image: Arc<RenderImage>,
    width: u32,
    height: u32,
}

impl Sheet {
    fn new(image: Arc<RenderImage>) -> Option<Self> {
        let size = image.size(0);
        image.as_bytes(0)?;
        Some(Self {
            width: size.width.0 as u32,
            height: size.height.0 as u32,
            image,
        })
    }

    fn contains_region(&self, x: u32, y: u32, width: u32, height: u32) -> bool {
        u64::from(x) + u64::from(width) <= u64::from(self.width)
            && u64::from(y) + u64::from(height) <= u64::from(self.height)
    }
}

/// Composes pieces in order with premultiplied source-over, then extends the
/// edges into the guard border. Bytes stay GPUI's straight-alpha BGRA.
fn compose(
    pieces: &[TilePiece],
    sheets: &[Option<Sheet>],
    width: u32,
    height: u32,
) -> Arc<RenderImage> {
    let mut tile: Vec<Pixel> = vec![[0.0; 4]; (width * height) as usize];
    for piece in pieces {
        let (piece_width, piece_height, left, top) = piece.get_bounds();
        let (sheet, fill) = match *piece {
            TilePiece::Sheet { sheet, x, y, .. } => {
                let source = sheets[sheet as usize]
                    .as_ref()
                    .expect("composed tiles use loaded sheets");
                (
                    Some((
                        source.image.as_bytes(0).expect("sheets hold pixels"),
                        source.width,
                        x,
                        y,
                    )),
                    [0.0; 4],
                )
            }
            TilePiece::Fill {
                fill: [r, g, b, a], ..
            } => (None, unpack(&[b, g, r, a])),
        };
        for y in 0..piece_height {
            for x in 0..piece_width {
                let source = match sheet {
                    Some((bytes, sheet_width, source_x, source_y)) => {
                        let offset = ((source_y + y) as usize * sheet_width as usize
                            + (source_x + x) as usize)
                            * 4;
                        unpack(&bytes[offset..offset + 4])
                    }
                    None => fill,
                };
                let target = ((top + y) * width + left + x) as usize;
                tile[target] = blend(source, tile[target], Blend::SourceOver);
            }
        }
    }
    let mut pixels = image::RgbaImage::new(width + 2 * GUARD, height + 2 * GUARD);
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        let tx = x.saturating_sub(GUARD).min(width - 1);
        let ty = y.saturating_sub(GUARD).min(height - 1);
        write(&mut pixel.0, tile[(ty * width + tx) as usize]);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_value, json};

    /// A 4 x 2 RGBA sheet: opaque red on the left half, half-transparent blue
    /// on the right half.
    fn assets() -> (tempfile::TempDir, AssetRoot) {
        let directory = tempfile::tempdir().unwrap();
        image::RgbaImage::from_fn(4, 2, |x, _| {
            image::Rgba(if x < 2 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 128]
            })
        })
        .save(directory.path().join("sheet.png"))
        .unwrap();
        let assets = AssetRoot::new(directory.path()).unwrap();
        (directory, assets)
    }

    fn tileset(value: serde_json::Value) -> Tileset {
        let tileset: Tileset = from_value(value).unwrap();
        tileset.validate().unwrap();
        tileset
    }

    fn pixel(image: &RenderImage, x: u32, y: u32) -> [u8; 4] {
        let width = image.size(0).width.0 as u32;
        let offset = ((y * width + x) * 4) as usize;
        image.as_bytes(0).unwrap()[offset..offset + 4]
            .try_into()
            .unwrap()
    }

    #[test]
    fn two_pieces_compose_in_order_with_a_clamped_guard() {
        let (_directory, assets) = assets();
        // Red fills the left half of a 2 x 2 tile; half-blue covers the whole
        // right column and blends over red where it overlaps the top-left.
        let tileset = tileset(
            json!({"tileSize":2,"sheets":["sheet.png"],"tiles":[{"frames":[[
                {"sheet":0,"x":0,"y":0,"width":1,"height":2,"left":0,"top":0},
                {"sheet":0,"x":2,"y":0,"width":1,"height":2,"left":1,"top":0},
                {"sheet":0,"x":3,"y":0,"width":1,"height":1,"left":0,"top":0}
            ]]}]}),
        );
        let (prepared, diagnostics) = PreparedTileset::prepare(&tileset, &assets);
        assert!(diagnostics.is_empty());
        let image = prepared.get_frame(0, 0).unwrap();
        let side = 2 + 2 * GUARD;
        assert_eq!(
            (image.size(0).width.0, image.size(0).height.0),
            (side as i32, side as i32)
        );
        // Bytes are BGRA with straight alpha.
        let red = [0, 0, 255, 255];
        let half_blue = [255, 0, 0, 128];
        // Blue at 128/255 over opaque red.
        let a: f32 = 128.0 / 255.0;
        let mixed = [
            (255.0 * a).round() as u8,
            0,
            (255.0 * (1.0 - a)).round() as u8,
            255,
        ];
        assert_eq!(pixel(image, GUARD, GUARD), mixed);
        assert_eq!(pixel(image, GUARD, GUARD + 1), red);
        assert_eq!(pixel(image, GUARD + 1, GUARD), half_blue);
        assert_eq!(pixel(image, GUARD + 1, GUARD + 1), half_blue);
        // The guard repeats the nearest edge pixel on every side.
        assert_eq!(pixel(image, 0, 0), mixed);
        assert_eq!(pixel(image, 0, side - 1), red);
        assert_eq!(pixel(image, side - 1, 0), half_blue);
        assert_eq!(pixel(image, side - 1, side - 1), half_blue);
        assert_eq!(prepared.images.len(), 1);
    }

    #[test]
    fn a_narrow_tile_composes_at_its_own_width() {
        let (_directory, assets) = assets();
        let tileset = tileset(
            json!({"tileSize":2,"sheets":["sheet.png"],"tiles":[{"width":1,"left":0,"frames":[[
                {"sheet":0,"x":2,"y":0,"width":1,"height":2,"left":0,"top":0}
            ]]}]}),
        );
        let (prepared, diagnostics) = PreparedTileset::prepare(&tileset, &assets);
        assert!(diagnostics.is_empty());
        let image = prepared.get_frame(0, 0).unwrap();
        assert_eq!(
            (image.size(0).width.0, image.size(0).height.0),
            ((1 + 2 * GUARD) as i32, (2 + 2 * GUARD) as i32)
        );
        assert_eq!(pixel(image, GUARD, GUARD), [255, 0, 0, 128]);
    }

    #[test]
    fn rgba_fills_compose_with_sheets_in_order_and_keep_alpha_and_guards() {
        let (_directory, assets) = assets();
        let tileset = tileset(json!({"tileSize":2,"sheets":["sheet.png"],"tiles":[
            {"frames":[[
                {"sheet":0,"x":0,"y":0,"width":2,"height":2,"left":0,"top":0},
                {"fill":[40,80,120,128],"width":1,"height":2,"left":0,"top":0}
            ]]},
            {"width":1,"frames":[[
                {"fill":[40,80,120,128],"width":1,"height":2,"left":0,"top":0}
            ]]},
            {"frames":[[
                {"fill":[40,80,120,128],"width":2,"height":2,"left":0,"top":0},
                {"sheet":0,"x":0,"y":0,"width":1,"height":2,"left":0,"top":0}
            ]]}
        ]}));
        let (prepared, diagnostics) = PreparedTileset::prepare(&tileset, &assets);
        assert!(diagnostics.is_empty());
        let blended = prepared.get_frame(0, 0).unwrap();
        assert_eq!(pixel(blended, GUARD, GUARD), [60, 40, 147, 255]);
        assert_eq!(pixel(blended, GUARD + 1, GUARD), [0, 0, 255, 255]);
        assert_eq!(pixel(blended, 0, 0), [60, 40, 147, 255]);
        let fill = prepared.get_frame(1, 0).unwrap();
        assert_eq!(pixel(fill, GUARD, GUARD), [120, 80, 40, 128]);
        assert_eq!(pixel(fill, 0, 0), [120, 80, 40, 128]);
        assert_eq!(
            pixel(prepared.get_frame(2, 0).unwrap(), GUARD, GUARD),
            [0, 0, 255, 255]
        );
        assert_eq!(prepared.get_frame(1, 99).unwrap().id, fill.id);
    }

    #[test]
    fn fills_stay_available_when_unrelated_sheet_tiles_cannot_load() {
        let (_directory, assets) = assets();
        let tileset = tileset(json!({"tileSize":2,"sheets":["missing.png"],"tiles":[
            {"frames":[[{"sheet":0,"x":0,"y":0,"width":2,"height":2,"left":0,"top":0}]]},
            {"width":1,"frames":[[{"fill":[0,0,0,64],"width":1,"height":2,"left":0,"top":0}]]}
        ]}));
        let (prepared, diagnostics) = PreparedTileset::prepare(&tileset, &assets);
        assert_eq!(diagnostics.len(), 1);
        assert!(!prepared.is_available(0));
        assert!(prepared.is_available(1));
        assert_eq!(
            pixel(prepared.get_frame(1, 0).unwrap(), GUARD, GUARD),
            [0, 0, 0, 64]
        );
    }
    #[test]
    fn missing_sheets_and_outside_pieces_leave_only_their_tiles_unavailable() {
        let (_directory, assets) = assets();
        let piece = |sheet: u32, x: u32| json!([{"sheet":sheet,"x":x,"y":0,"width":2,"height":2,"left":0,"top":0}]);
        let tileset = tileset(json!({"tileSize":2,
        "sheets":["sheet.png","missing.png","../escape.png"],
        "tiles":[
            {"frames":[piece(0,0)]},
            {"frames":[piece(1,0)]},
            {"frames":[piece(0,0), piece(2,0)]},
            {"frames":[piece(0,3)]},
            {"frames":[piece(0,4)]},
            {"frames":[piece(0,2), piece(0,0)]}
        ]}));
        let (prepared, diagnostics) = PreparedTileset::prepare(&tileset, &assets);
        let available: Vec<_> = (0..7).map(|tile| prepared.is_available(tile)).collect();
        assert_eq!(available, [true, false, false, false, false, true, false]);
        // One diagnostic per unusable sheet, however many pieces use it.
        assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
        assert!(diagnostics[0].contains("missing.png"));
        assert!(diagnostics[1].contains("escape.png"));
        assert!(diagnostics[2].contains("sheet.png") && diagnostics[2].contains('2'));
        assert!(prepared.get_frame(1, 0).is_none());
    }

    #[test]
    fn a_tile_shows_its_frame_for_the_animation_counter() {
        let (_directory, assets) = assets();
        let frame = |x: u32| json!([{"sheet":0,"x":x,"y":0,"width":2,"height":2,"left":0,"top":0}]);
        let tileset = tileset(json!({"tileSize":2,"sheets":["sheet.png"],"tiles":[
            {"frames":[frame(0), frame(2), frame(1)]},
            {"frames":[frame(2)]}
        ]}));
        let (prepared, _) = PreparedTileset::prepare(&tileset, &assets);
        let first = prepared.get_frame(0, 0).unwrap().id;
        let second = prepared.get_frame(0, 1).unwrap().id;
        let third = prepared.get_frame(0, 2).unwrap().id;
        assert!(first != second && second != third && first != third);
        assert_eq!(prepared.get_frame(0, 3).unwrap().id, first);
        assert_eq!(prepared.get_frame(0, 7).unwrap().id, second);
        assert_eq!(prepared.get_frame(0, u32::MAX).unwrap().id, first);
        let still = prepared.get_frame(1, 0).unwrap().id;
        assert_eq!(prepared.get_frame(1, 5).unwrap().id, still);
        assert!(prepared.get_frame(2, 0).is_none());
        assert_eq!(select_frame(0, 3), None);
        assert_eq!(select_frame(4, 6), Some(2));
    }

    #[test]
    fn composition_stops_at_its_byte_bound() {
        let (_directory, assets) = assets();
        // A 256-pixel tile frame with its guard is 260 * 260 * 4 bytes, so
        // the bound admits 248 of them; later tiles are not composed.
        let frame = json!([{"sheet":0,"x":0,"y":0,"width":1,"height":1,"left":0,"top":0}]);
        let tiles: Vec<_> = (0..63)
            .map(|_| json!({"frames":[frame, frame, frame, frame]}))
            .collect();
        let tileset = tileset(json!({"tileSize":256,"sheets":["sheet.png"],"tiles":tiles}));
        let (prepared, diagnostics) = PreparedTileset::prepare(&tileset, &assets);
        let admitted = MAX_COMPOSED_BYTES / (260 * 260 * 4) / 4;
        assert_eq!(admitted, 62);
        assert!(prepared.is_available(admitted as u32 - 1));
        assert!(!prepared.is_available(admitted as u32));
        assert_eq!(diagnostics.len(), 1);
        assert!(
            prepared
                .images
                .iter()
                .map(|image| image.as_bytes(0).unwrap().len())
                .sum::<usize>()
                <= MAX_COMPOSED_BYTES
        );
    }
}
