//! Negotiated graphical coordinates, independent of the terminal grid.
use crate::color::ColorSpec;
use crate::protocol::{
    Grid, MAX_TEXT_LAYERS, MAX_TEXT_RUNS, MAX_TEXT_SCALARS, SourceRect, TextRun,
};
use serde::Deserialize;
use std::{collections::HashSet, path::PathBuf};

pub const MAX_IMAGES: usize = 1024;
pub const MAX_INDICATORS: usize = 2048;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub images: Vec<CanvasImage>,
    #[serde(default)]
    pub indicators: Vec<Indicator>,
    #[serde(default)]
    pub text_layers: Vec<CanvasText>,
}

pub fn optional_canvas<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Canvas>, D::Error> {
    // Omission clears; explicit null must not become an implicit empty frame.
    Canvas::deserialize(d).map(Some)
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn validate(self, canvas: &Canvas) -> Result<(), String> {
        if ![self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f64::is_finite)
            || self.x < 0.0
            || self.y < 0.0
            || self.width <= 0.0
            || self.height <= 0.0
            || !(self.x + self.width).is_finite()
            || !(self.y + self.height).is_finite()
            || self.x + self.width > f64::from(canvas.width)
            || self.y + self.height > f64::from(canvas.height)
        {
            return Err("canvas rectangle must be finite, positive and within the canvas".into());
        }
        Ok(())
    }

    pub fn paint(
        self,
        transform: crate::viewport::ViewportTransform,
    ) -> crate::viewport::PaintRect {
        transform.surface_rect(
            self.x as f32,
            self.y as f32,
            self.width as f32,
            self.height as f32,
        )
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CanvasImage {
    pub id: String,
    pub asset: PathBuf,
    pub destination: Rect,
    pub layer: i32,
    #[serde(default, deserialize_with = "crate::protocol::source_rect")]
    pub source_rect: Option<SourceRect>,
    #[serde(default = "opaque")]
    pub opacity: f64,
}

fn opaque() -> f64 {
    1.0
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IndicatorKind {
    Outline,
    Underline,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Indicator {
    pub id: String,
    pub image_id: String,
    pub kind: IndicatorKind,
    pub bounds: Rect,
    pub stroke_width: u32,
    pub color: ColorSpec,
    pub layer: i32,
}

impl Indicator {
    /// Nonoverlapping inside strokes. No target selection or actor layout lives here.
    pub fn strokes(&self) -> Vec<Rect> {
        let Rect {
            x,
            y,
            width,
            height,
        } = self.bounds;
        let s = f64::from(self.stroke_width);
        if self.kind == IndicatorKind::Underline {
            return vec![Rect {
                x,
                y: y + height - s,
                width,
                height: s,
            }];
        }
        // Very small rectangles may be entirely filled by their inside border.
        if 2.0 * s >= width || 2.0 * s >= height {
            return vec![self.bounds];
        }
        vec![
            Rect {
                x,
                y,
                width,
                height: s,
            },
            Rect {
                x,
                y: y + height - s,
                width,
                height: s,
            },
            Rect {
                x,
                y: y + s,
                width: s,
                height: height - 2.0 * s,
            },
            Rect {
                x: x + width - s,
                y: y + s,
                width: s,
                height: height - 2.0 * s,
            },
        ]
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CanvasText {
    pub id: String,
    pub origin: Origin,
    pub grid: Grid,
    pub runs: Vec<TextRun>,
    pub layer: i32,
}

fn identifiers<'a>(ids: impl Iterator<Item = &'a str>) -> Result<(), String> {
    let mut seen = HashSet::new();
    for id in ids {
        if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) || !seen.insert(id) {
            return Err(
                "canvas ids must be unique, nonempty, control-free and at most 256 UTF-8 bytes"
                    .into(),
            );
        }
    }
    Ok(())
}

impl Canvas {
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 || self.width > 16384 || self.height > 16384 {
            return Err("canvas dimensions must be integers in 1..16384".into());
        }
        if self.images.len() > MAX_IMAGES || self.indicators.len() > MAX_INDICATORS {
            return Err("canvas exceeds 1024 images or 2048 indicators".into());
        }
        if self.text_layers.len() > MAX_TEXT_LAYERS {
            return Err("canvas exceeds 64 text layers".into());
        }
        identifiers(self.images.iter().map(|i| i.id.as_str()))?;
        identifiers(self.indicators.iter().map(|i| i.id.as_str()))?;
        identifiers(self.text_layers.iter().map(|i| i.id.as_str()))?;
        for image in &self.images {
            image.destination.validate(self)?;
            if image.asset.as_os_str().is_empty()
                || image.asset.is_absolute()
                || image.asset.as_os_str().len() > 4096
            {
                return Err(
                    "canvas asset must be a nonempty relative path of at most 4096 UTF-8 bytes"
                        .into(),
                );
            }
            if !image.opacity.is_finite() || !(0.0..=1.0).contains(&image.opacity) {
                return Err("canvas opacity must be finite and between 0 and 1".into());
            }
            if let Some(rect) = image.source_rect {
                rect.validate()?;
            }
        }
        let images: HashSet<_> = self.images.iter().map(|i| &i.id).collect();
        for indicator in &self.indicators {
            indicator.bounds.validate(self)?;
            if !images.contains(&indicator.image_id) {
                return Err(
                    "canvas indicator imageId must reference an image in this frame".into(),
                );
            }
            if !(1..=16).contains(&indicator.stroke_width)
                || f64::from(indicator.stroke_width)
                    > indicator.bounds.width.min(indicator.bounds.height)
            {
                return Err("canvas strokeWidth must be in 1..16 and fit inside its bounds".into());
            }
        }
        let mut runs = 0usize;
        let mut scalars = 0usize;
        for layer in &self.text_layers {
            layer.grid.validate()?;
            Rect {
                x: layer.origin.x,
                y: layer.origin.y,
                width: f64::from(layer.grid.columns * layer.grid.cell_width),
                height: f64::from(layer.grid.rows * layer.grid.cell_height),
            }
            .validate(self)?;
            runs += layer.runs.len();
            if runs > MAX_TEXT_RUNS {
                return Err("canvas exceeds 32768 text runs".into());
            }
            for run in &layer.runs {
                let count = run.text.chars().count();
                scalars += count;
                if scalars > MAX_TEXT_SCALARS {
                    return Err("canvas exceeds 524288 text scalars".into());
                }
                if run.row >= layer.grid.rows
                    || run.column >= layer.grid.columns
                    || count > (layer.grid.columns - run.column) as usize
                {
                    return Err("canvas text run exceeds its local grid".into());
                }
                if run.text.chars().any(char::is_control) {
                    return Err("canvas text must not contain control characters".into());
                }
            }
        }
        Ok(())
    }
}
