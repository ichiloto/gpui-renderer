//! Dark-terminal base palette; the xterm cube/grayscale and explicit RGB stay literal.
use serde::Deserialize;

pub const DEFAULT_BACKGROUND: u32 = 0x111820;
pub const DEFAULT_FOREGROUND: u32 = 0xd9e1e8;
pub const ANSI16: [u32; 16] = [
    0x000000, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xc5ced8, 0x7f8c9a,
    0xff8c95, 0xb3e38f, 0xffdfa3, 0x8ccaff, 0xe5a3f5, 0x83dce5, 0xffffff,
];

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ColorSpec {
    Ansi16 {
        #[serde(deserialize_with = "ansi16_index")]
        index: u8,
    },
    Ansi256 {
        index: u8,
    },
    Rgb {
        r: u8,
        g: u8,
        b: u8,
    },
}

fn ansi16_index<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    let index = u8::deserialize(d)?;
    if index > 15 {
        return Err(serde::de::Error::custom("ansi16 index must be 0..15"));
    }
    Ok(index)
}

impl ColorSpec {
    pub fn rgb(&self) -> u32 {
        match *self {
            Self::Ansi16 { index } => ANSI16[usize::from(index)],
            Self::Ansi256 {
                index: index @ 0..=15,
            } => ANSI16[usize::from(index)],
            Self::Ansi256 {
                index: index @ 16..=231,
            } => {
                let levels = [0, 95, 135, 175, 215, 255];
                let n = usize::from(index - 16);
                (levels[n / 36] << 16) | (levels[n / 6 % 6] << 8) | levels[n % 6]
            }
            Self::Ansi256 { index } => u32::from(8 + (index - 232) * 10) * 0x010101,
            Self::Rgb { r, g, b } => (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn luminance(rgb: u32) -> f64 {
        [16, 8, 0]
            .into_iter()
            .zip([0.2126, 0.7152, 0.0722])
            .map(|(shift, weight)| {
                let channel = f64::from((rgb >> shift) & 255) / 255.0;
                weight
                    * if channel <= 0.04045 {
                        channel / 12.92
                    } else {
                        ((channel + 0.055) / 1.055).powf(2.4)
                    }
            })
            .sum()
    }

    fn contrast(a: u32, b: u32) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn dark_terminal_palette_is_readable_and_bright_variants_remain_distinct() {
        // Black stays a literal dark colour, especially for explicitly authored backgrounds.
        assert_eq!(ANSI16[0], 0);
        for (index, &colour) in ANSI16.iter().enumerate().skip(1) {
            let ratio = contrast(colour, DEFAULT_BACKGROUND);
            assert!(
                ratio >= 4.5,
                "ANSI{index} contrast {ratio:.3}:1 is too low on the default background"
            );
        }
        for normal in 1..=7 {
            assert!(luminance(ANSI16[normal + 8]) > luminance(ANSI16[normal]));
        }
        assert!(contrast(DEFAULT_FOREGROUND, DEFAULT_BACKGROUND) >= 7.0);
        for (index, old) in [(4, 0x000080), (12, 0x0000ff)] {
            assert!(
                contrast(ANSI16[index], DEFAULT_BACKGROUND)
                    > 3.0 * contrast(old, DEFAULT_BACKGROUND)
            );
        }
        // Authored cube/RGB blues are not dynamically brightened to meet this theme target.
        assert_eq!(ColorSpec::Ansi256 { index: 21 }.rgb(), 0x0000ff);
        assert_eq!(ColorSpec::Rgb { r: 0, g: 0, b: 128 }.rgb(), 0x000080);
    }
    #[test]
    fn tagged_colours_are_strict() {
        for index in 0..=15 {
            assert!(
                serde_json::from_value::<ColorSpec>(json!({"kind":"ansi16","index":index})).is_ok()
            );
        }
        for index in 0..=255 {
            assert!(
                serde_json::from_value::<ColorSpec>(json!({"kind":"ansi256","index":index}))
                    .is_ok()
            );
        }
        for invalid in [
            json!({"kind":"ansi16","index":16}),
            json!({"kind":"ansi16","index":-1}),
            json!({"kind":"ansi256","index":256}),
            json!({"kind":"ansi256","index":1.5}),
            json!({"kind":"rgb","r":256,"g":0,"b":0}),
            json!({"kind":"rgb","r":0,"g":-1,"b":0}),
            json!({"kind":"rgb","r":0,"g":0,"b":"255"}),
            json!({"kind":"rgb","r":0,"g":0}),
            json!({"kind":"ansi16","index":1,"extra":0}),
            json!({"kind":"css","value":"red"}),
            json!("#fff"),
        ] {
            assert!(
                serde_json::from_value::<ColorSpec>(invalid.clone()).is_err(),
                "{invalid}"
            );
        }
    }
    #[test]
    fn palette_cube_grayscale_and_rgb_are_deterministic() {
        for index in 0..16 {
            assert_eq!(ColorSpec::Ansi256 { index }.rgb(), ANSI16[index as usize]);
        }
        for (index, rgb) in [
            (16, 0),
            (17, 0x00005f),
            (21, 0x0000ff),
            (46, 0x00ff00),
            (196, 0xff0000),
            (231, 0xffffff),
            (232, 0x080808),
            (255, 0xeeeeee),
        ] {
            assert_eq!(ColorSpec::Ansi256 { index }.rgb(), rgb);
        }
        assert_eq!(
            ColorSpec::Rgb {
                r: 255,
                g: 135,
                b: 175
            }
            .rgb(),
            0xff87af
        );
    }
}
