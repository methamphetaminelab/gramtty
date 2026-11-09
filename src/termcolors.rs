use std::sync::RwLock;

pub type Rgb = (u8, u8, u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TermPalette {
    pub fg: Rgb,
    pub bg: Rgb,
    pub ansi: [Rgb; 16],
}

pub const FALLBACK: TermPalette = TermPalette {
    fg: (220, 222, 228),
    bg: (22, 24, 30),
    ansi: [
        (40, 42, 54),
        (224, 108, 117),
        (152, 195, 121),
        (229, 192, 123),
        (97, 175, 239),
        (198, 120, 221),
        (86, 182, 194),
        (171, 178, 191),
        (92, 99, 112),
        (240, 130, 140),
        (170, 215, 140),
        (240, 210, 150),
        (120, 190, 250),
        (215, 140, 235),
        (110, 200, 210),
        (230, 232, 238),
    ],
};

static CURRENT: RwLock<Option<TermPalette>> = RwLock::new(None);

pub fn get() -> Option<TermPalette> {
    *CURRENT.read().unwrap_or_else(|e| e.into_inner())
}

pub fn set(p: Option<TermPalette>) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = p;
}

pub fn base() -> TermPalette {
    get().unwrap_or(FALLBACK)
}

pub fn parse_spec(raw: &str) -> Option<Rgb> {
    let raw = raw.trim();
    if let Some(hex) = raw.strip_prefix('#') {
        if !hex.is_ascii() || hex.len() % 3 != 0 || hex.is_empty() || hex.len() > 12 {
            return None;
        }
        let n = hex.len() / 3;
        let parts: Vec<&str> = (0..3).map(|i| &hex[i * n..(i + 1) * n]).collect();
        return channels(&parts);
    }
    let body = raw
        .strip_prefix("rgba:")
        .or_else(|| raw.strip_prefix("rgb:"))?;
    let parts: Vec<&str> = body.split('/').collect();
    if parts.len() < 3 {
        return None;
    }
    channels(&parts[..3])
}

fn channels(parts: &[&str]) -> Option<Rgb> {
    let mut out = [0u8; 3];
    for (o, p) in out.iter_mut().zip(parts) {
        if p.is_empty() || p.len() > 4 {
            return None;
        }
        let v = u32::from_str_radix(p, 16).ok()?;
        let max = (1u32 << (4 * p.len() as u32)) - 1;
        *o = ((v * 255 + max / 2) / max) as u8;
    }
    Some((out[0], out[1], out[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_in_all_xparsecolor_widths_parse() {
        assert_eq!(parse_spec("rgb:ffff/8080/0000"), Some((255, 128, 0)));
        assert_eq!(parse_spec("rgb:ff/80/00"), Some((255, 128, 0)));
        assert_eq!(parse_spec("rgb:f/8/0"), Some((255, 136, 0)));
        assert_eq!(parse_spec("rgba:1e1e/2020/2a2a/ffff"), Some((30, 32, 42)));
        assert_eq!(parse_spec("#1e202a"), Some((30, 32, 42)));
        assert_eq!(parse_spec("rgb:zz/00/00"), None);
        assert_eq!(parse_spec("rgb:00/00"), None);
        assert_eq!(parse_spec("blue"), None);
        assert_eq!(parse_spec("#\u{FFFD}"), None);
        assert_eq!(parse_spec("#a\u{e9}bcd"), None);
        assert_eq!(parse_spec("rgb:\u{FFFD}/00/00"), None);
    }
}
