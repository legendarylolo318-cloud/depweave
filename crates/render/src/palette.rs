//! Colour helpers. Okabe–Ito-inspired, readable on dark and light backgrounds.

pub type Rgb = [u8; 3];

pub const EXPLICIT: Rgb = [230, 159, 0];
pub const DEPENDENCY: Rgb = [86, 160, 224];
pub const ORPHAN: Rgb = [226, 85, 60];

pub const CORE: Rgb = [76, 155, 232];
pub const EXTRA: Rgb = [61, 187, 140];
pub const MULTILIB: Rgb = [230, 162, 60];
pub const FOREIGN: Rgb = [216, 111, 166];
const CUSTOM: [Rgb; 4] = [
    [167, 139, 250],
    [95, 208, 216],
    [228, 215, 91],
    [240, 120, 90],
];

pub fn repo_color(repo: Option<&str>, custom_index: usize) -> Rgb {
    match repo {
        None => FOREIGN,
        Some("core") => CORE,
        Some("extra") => EXTRA,
        Some("multilib") => MULTILIB,
        Some(_) => CUSTOM[custom_index % CUSTOM.len()],
    }
}

/// Sequential gradient (dark purple → teal → yellow), `t` in 0..=1.
pub fn gradient(t: f32) -> Rgb {
    const STOPS: [Rgb; 5] = [
        [68, 1, 84],
        [59, 82, 139],
        [33, 144, 140],
        [93, 201, 99],
        [253, 231, 37],
    ];
    let t = t.clamp(0.0, 1.0) * (STOPS.len() - 1) as f32;
    let i = (t.floor() as usize).min(STOPS.len() - 2);
    let f = t - i as f32;
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * f).round() as u8;
    [
        mix(STOPS[i][0], STOPS[i + 1][0]),
        mix(STOPS[i][1], STOPS[i + 1][1]),
        mix(STOPS[i][2], STOPS[i + 1][2]),
    ]
}

/// Log-scaled size → 0..=1 given the max size in the data set.
pub fn size_t(size: u64, max: u64) -> f32 {
    if max == 0 {
        return 0.0;
    }
    ((size as f32 + 1.0).ln() / (max as f32 + 1.0).ln()).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_endpoints() {
        assert_eq!(gradient(0.0), [68, 1, 84]);
        assert_eq!(gradient(1.0), [253, 231, 37]);
        assert_eq!(gradient(7.0), gradient(1.0));
        assert_eq!(size_t(0, 0), 0.0);
        assert!(size_t(1000, 1_000_000) < size_t(1_000_000, 1_000_000));
    }
}
