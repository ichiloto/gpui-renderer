//! Deterministic xterm palette. Wire colours are identities, never ANSI/CSS.
use serde::Deserialize;

pub const DEFAULT_BACKGROUND: u32 = 0x111820;
pub const DEFAULT_FOREGROUND: u32 = 0xd9e1e8;
pub const ANSI16: [u32; 16] = [
    0x000000, 0x800000, 0x008000, 0x808000, 0x000080, 0x800080, 0x008080, 0xc0c0c0, 0x808080,
    0xff0000, 0x00ff00, 0xffff00, 0x0000ff, 0xff00ff, 0x00ffff, 0xffffff,
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
