//! Strict graphical text effects. Bounds and motion remain Engine-owned.
use crate::{canvas_protocol::Rect, color::ColorSpec};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GlyphEffects {
    pub outline: Outline,
    pub shadow: Shadow,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Outline {
    pub width: f64,
    pub color: ColorSpec,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Shadow {
    pub offset_x: f64,
    pub offset_y: f64,
    pub sigma: f64,
    pub opacity: f64,
    pub color: ColorSpec,
}

impl GlyphEffects {
    pub fn validate(&self) -> Result<(), String> {
        let s = &self.shadow;
        if ![
            self.outline.width,
            s.offset_x,
            s.offset_y,
            s.sigma,
            s.opacity,
        ]
        .into_iter()
        .all(f64::is_finite)
            || !(0.0..=4.0).contains(&self.outline.width)
            || !(-8.0..=8.0).contains(&s.offset_x)
            || !(-8.0..=8.0).contains(&s.offset_y)
            || !(0.0..=4.0).contains(&s.sigma)
            || !(0.0..=1.0).contains(&s.opacity)
        {
            return Err("glyphEffects exceed finite contour/shadow limits".into());
        }
        Ok(())
    }

    /// Left, top, right, bottom. Identical to Engine CanvasGlyphEffects::padding.
    pub fn padding(&self) -> [f64; 4] {
        let radius = self.outline.width + (3.0 * self.shadow.sigma).ceil();
        [
            (radius + (-self.shadow.offset_x).max(0.0)).ceil(),
            (radius + (-self.shadow.offset_y).max(0.0)).ceil(),
            (radius + self.shadow.offset_x.max(0.0)).ceil(),
            (radius + self.shadow.offset_y.max(0.0)).ceil(),
        ]
    }

    pub fn expand(&self, bounds: Rect) -> Rect {
        let [left, top, right, bottom] = self.padding();
        Rect {
            x: bounds.x - left,
            y: bounds.y - top,
            width: bounds.width + left + right,
            height: bounds.height + top + bottom,
        }
    }
}
