//! Bounded, data-only local composition. Animation and artwork registration belong to PHP.
use crate::{
    canvas_protocol::{Canvas, Rect, present},
    color::ColorSpec,
};
use serde::Deserialize;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

pub const MAX_COMPOSITES: usize = 8;
pub const MAX_PIXELS: usize = 8_388_608;
pub const MAX_OPERATIONS: usize = 256;
pub const MAX_NODES: usize = 16_384;
pub const MAX_WORK: usize = 256 * 1024 * 1024;
pub type Point = [f64; 2];
fn opaque() -> f64 {
    1.0
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Composite {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub destination: Rect,
    pub layer: i32,
    #[serde(default = "opaque")]
    pub opacity: f64,
    #[serde(default, deserialize_with = "present")]
    pub clip_rect: Option<Rect>,
    pub operations: Vec<Operation>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Blend {
    #[default]
    SourceOver,
    Screen,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Image {
        asset: PathBuf,
        #[serde(default, deserialize_with = "present")]
        source: Option<Rect>,
        destination: Rect,
        #[serde(default = "opaque")]
        opacity: f64,
        #[serde(default)]
        blend: Blend,
        #[serde(default)]
        masks: Vec<Mask>,
        #[serde(default, deserialize_with = "present")]
        displacement: Option<Displacement>,
    },
    Fill {
        destination: Rect,
        brush: Brush,
        #[serde(default = "opaque")]
        opacity: f64,
        #[serde(default)]
        blend: Blend,
        #[serde(default)]
        masks: Vec<Mask>,
    },
    Stroke {
        points: Vec<Point>,
        width: f64,
        brush: Brush,
        #[serde(default = "opaque")]
        opacity: f64,
        #[serde(default)]
        blend: Blend,
        #[serde(default)]
        masks: Vec<Mask>,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Displacement {
    pub columns: u32,
    pub rows: u32,
    pub offsets: Vec<Point>,
    #[serde(default)]
    pub masks: Vec<Mask>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Brush {
    Solid {
        color: ColorSpec,
    },
    Linear {
        start: Point,
        end: Point,
        stops: Vec<Stop>,
    },
    Radial {
        center: Point,
        radius: Point,
        stops: Vec<Stop>,
    },
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Stop {
    pub offset: f64,
    pub color: ColorSpec,
    #[serde(default = "opaque")]
    pub opacity: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mask {
    Polygon {
        contours: Vec<Vec<Point>>,
        #[serde(default)]
        feather: f64,
        #[serde(default)]
        invert: bool,
    },
    Ellipse {
        center: Point,
        radius: Point,
        #[serde(default)]
        feather: f64,
        #[serde(default)]
        invert: bool,
    },
    ImageAlpha {
        asset: PathBuf,
        #[serde(default, deserialize_with = "present")]
        source: Option<Rect>,
        destination: Rect,
        #[serde(default)]
        invert: bool,
    },
}

pub fn whole_source() -> Rect {
    Rect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    }
}
fn finite(value: f64, min: f64, max: f64) -> bool {
    value.is_finite() && (min..=max).contains(&value)
}
fn point(p: Point) -> bool {
    p.into_iter().all(|v| finite(v, -16384.0, 16384.0))
}
fn radius(r: Point) -> bool {
    r.into_iter().all(|v| finite(v, f64::MIN_POSITIVE, 16384.0))
}
fn asset(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() || path.is_absolute() || path.as_os_str().len() > 4096 {
        return Err(
            "composite asset must be a nonempty relative path of at most 4096 bytes".into(),
        );
    }
    Ok(())
}
fn source(rect: Option<Rect>) -> Result<(), String> {
    let r = rect.unwrap_or_else(whole_source);
    if ![r.x, r.y, r.width, r.height]
        .into_iter()
        .all(|v| finite(v, 0.0, 1.0))
        || r.width <= 0.0
        || r.height <= 0.0
        || r.x + r.width > 1.0
        || r.y + r.height > 1.0
    {
        return Err("composite source must be a positive normalized rectangle within [0,1]".into());
    }
    Ok(())
}
fn rect(r: Rect, width: u32, height: u32) -> Result<(), String> {
    if ![r.x, r.y, r.width, r.height]
        .into_iter()
        .all(f64::is_finite)
        || r.x < 0.0
        || r.y < 0.0
        || r.width <= 0.0
        || r.height <= 0.0
        || r.x + r.width > width as f64
        || r.y + r.height > height as f64
    {
        return Err("composite destination must fit its local target".into());
    }
    Ok(())
}
impl Brush {
    fn validate(&self) -> Result<(), String> {
        let stops = match self {
            Self::Solid { .. } => return Ok(()),
            Self::Linear { start, end, stops } => {
                if !point(*start) || !point(*end) || start == end {
                    return Err("invalid linear gradient axis".into());
                }
                stops
            }
            Self::Radial {
                center,
                radius: r,
                stops,
            } => {
                if !point(*center) || !radius(*r) {
                    return Err("invalid radial gradient geometry".into());
                }
                stops
            }
        };
        if !(2..=8).contains(&stops.len())
            || stops.first().unwrap().offset != 0.0
            || stops.last().unwrap().offset != 1.0
            || stops
                .iter()
                .any(|s| !finite(s.offset, 0.0, 1.0) || !finite(s.opacity, 0.0, 1.0))
            || stops.windows(2).any(|p| p[0].offset >= p[1].offset)
        {
            return Err("gradient requires 2..8 increasing stops from 0 to 1".into());
        }
        Ok(())
    }
}
impl Mask {
    pub fn get_work(&self) -> usize {
        match self {
            Self::Polygon { contours, .. } => contours.iter().map(Vec::len).sum(),
            Self::Ellipse { .. } => 8,
            Self::ImageAlpha { .. } => 16,
        }
    }
    fn validate(&self, width: u32, height: u32) -> Result<(), String> {
        let feather = match self {
            Self::Polygon {
                contours, feather, ..
            } => {
                if contours.is_empty()
                    || contours.len() > 8
                    || contours.iter().any(|p| {
                        !(3..=128).contains(&p.len())
                            || p.iter().any(|p| !point(*p))
                            || p.iter()
                                .enumerate()
                                .any(|(i, a)| *a == p[(i + 1) % p.len()])
                    })
                {
                    return Err("invalid composite polygon contours".into());
                }
                *feather
            }
            Self::Ellipse {
                center,
                radius: r,
                feather,
                ..
            } => {
                if !point(*center) || !radius(*r) {
                    return Err("invalid ellipse mask".into());
                }
                *feather
            }
            Self::ImageAlpha {
                asset: a,
                source: s,
                destination,
                ..
            } => {
                asset(a)?;
                source(*s)?;
                rect(*destination, width, height)?;
                0.0
            }
        };
        if !finite(feather, 0.0, 4096.0) {
            return Err("mask feather must be in 0..4096".into());
        }
        Ok(())
    }
}
fn masks(list: &[Mask], width: u32, height: u32) -> Result<(), String> {
    if list.len() > 8 {
        return Err("at most eight masks per operation or displacement".into());
    }
    for m in list {
        m.validate(width, height)?;
    }
    Ok(())
}
impl Operation {
    pub fn get_masks(&self) -> &[Mask] {
        match self {
            Self::Image { masks, .. } | Self::Fill { masks, .. } | Self::Stroke { masks, .. } => {
                masks
            }
        }
    }
    pub fn get_opacity(&self) -> f64 {
        match self {
            Self::Image { opacity, .. }
            | Self::Fill { opacity, .. }
            | Self::Stroke { opacity, .. } => *opacity,
        }
    }
    pub fn get_blend(&self) -> Blend {
        match self {
            Self::Image { blend, .. } | Self::Fill { blend, .. } | Self::Stroke { blend, .. } => {
                *blend
            }
        }
    }
    pub fn get_bounds(&self) -> Rect {
        match self {
            Self::Image { destination, .. } | Self::Fill { destination, .. } => *destination,
            Self::Stroke { points, width, .. } => {
                let left =
                    points.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min) - width / 2.0 - 0.5;
                let top =
                    points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min) - width / 2.0 - 0.5;
                Rect {
                    x: left,
                    y: top,
                    width: points
                        .iter()
                        .map(|p| p[0])
                        .fold(f64::NEG_INFINITY, f64::max)
                        + width / 2.0
                        + 0.5
                        - left,
                    height: points
                        .iter()
                        .map(|p| p[1])
                        .fold(f64::NEG_INFINITY, f64::max)
                        + width / 2.0
                        + 0.5
                        - top,
                }
            }
        }
    }
    pub fn get_assets(&self) -> Vec<&Path> {
        let mut paths = Vec::new();
        let mut mask_lists = vec![self.get_masks()];
        if let Self::Image {
            asset,
            displacement,
            ..
        } = self
        {
            paths.push(asset.as_path());
            if let Some(d) = displacement {
                mask_lists.push(&d.masks);
            }
        }
        for mask in mask_lists.into_iter().flatten() {
            if let Mask::ImageAlpha { asset, .. } = mask {
                paths.push(asset.as_path());
            }
        }
        paths
    }
    fn validate(&self, width: u32, height: u32) -> Result<usize, String> {
        if !finite(self.get_opacity(), 0.0, 1.0) {
            return Err("composite opacity must be in 0..1".into());
        }
        masks(self.get_masks(), width, height)?;
        match self {
            Self::Image {
                asset: a,
                source: s,
                destination,
                displacement,
                ..
            } => {
                asset(a)?;
                source(*s)?;
                rect(*destination, width, height)?;
                if let Some(d) = displacement {
                    if !(2..=64).contains(&d.columns)
                        || !(2..=64).contains(&d.rows)
                        || d.offsets.len() != (d.columns * d.rows) as usize
                        || d.offsets
                            .iter()
                            .flatten()
                            .any(|v| !finite(*v, -4096.0, 4096.0))
                    {
                        return Err("invalid displacement grid".into());
                    }
                    masks(&d.masks, width, height)?;
                    return Ok(d.offsets.len());
                }
            }
            Self::Fill {
                destination, brush, ..
            } => {
                rect(*destination, width, height)?;
                brush.validate()?;
            }
            Self::Stroke {
                points,
                width,
                brush,
                ..
            } => {
                if !(2..=256).contains(&points.len())
                    || points.iter().any(|p| !point(*p))
                    || !finite(*width, f64::MIN_POSITIVE, 256.0)
                {
                    return Err("invalid stroke geometry".into());
                }
                brush.validate()?;
            }
        }
        Ok(0)
    }
}

pub fn validate(composites: &[Composite], canvas: &Canvas) -> Result<(), String> {
    if composites.len() > MAX_COMPOSITES {
        return Err("frame exceeds eight composites".into());
    }
    let mut ids = HashSet::new();
    let (mut pixels, mut operations, mut nodes) = (0usize, 0usize, 0usize);
    for c in composites {
        if c.id.is_empty()
            || c.id.len() > 256
            || c.id.chars().any(char::is_control)
            || !ids.insert(&c.id)
        {
            return Err("composite ids must be unique, nonempty and at most 256 bytes".into());
        }
        if c.width == 0
            || c.height == 0
            || c.width > 4096
            || c.height > 4096
            || u64::from(c.width) * u64::from(c.height) > 4_194_304
        {
            return Err("composite target exceeds 4096 axes or 4194304 pixels".into());
        }
        c.destination.validate(canvas)?;
        if let Some(clip) = c.clip_rect {
            clip.validate(canvas)?;
        }
        if !finite(c.opacity, 0.0, 1.0) {
            return Err("composite opacity must be in 0..1".into());
        }
        pixels += c.width as usize * c.height as usize;
        operations += c.operations.len();
        for op in &c.operations {
            nodes += op.validate(c.width, c.height)?;
        }
    }
    if pixels > MAX_PIXELS || operations > MAX_OPERATIONS || nodes > MAX_NODES {
        return Err("composite frame exceeds pixel, operation or displacement-node budget".into());
    }
    Ok(())
}
