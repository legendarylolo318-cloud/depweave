//! Glyph rasterisation via fontdue with a small cache.

use std::collections::HashMap;
use std::path::PathBuf;

pub struct TextEngine {
    font: Option<fontdue::Font>,
    cache: HashMap<(char, u32), (fontdue::Metrics, Vec<u8>)>,
}

fn candidate_fonts() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os("DEPWEAVE_FONT") {
        v.push(PathBuf::from(p));
    }
    for root in ["/usr/share/fonts", "/usr/local/share/fonts"] {
        for rel in [
            "TTF/DejaVuSans.ttf",
            "noto/NotoSans-Regular.ttf",
            "liberation/LiberationSans-Regular.ttf",
            "inter/Inter-Regular.otf",
            "TTF/Inter-Regular.ttf",
            "cantarell/Cantarell-VF.otf",
        ] {
            v.push(PathBuf::from(root).join(rel));
        }
    }
    v
}

/// Last resort: first .ttf/.otf found under the system font dirs.
fn scan_any() -> Option<PathBuf> {
    let mut stack = vec![PathBuf::from("/usr/share/fonts")];
    let mut seen = 0;
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            seen += 1;
            if seen > 5000 {
                return None;
            }
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "ttf" || x == "otf") {
                return Some(p);
            }
        }
    }
    None
}

impl TextEngine {
    pub fn new() -> Self {
        let mut font = None;
        let mut paths = candidate_fonts();
        paths.extend(scan_any());
        for p in paths {
            if let Ok(bytes) = std::fs::read(&p) {
                if let Ok(f) = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()) {
                    log::info!("label font: {}", p.display());
                    font = Some(f);
                    break;
                }
            }
        }
        if font.is_none() {
            log::warn!("no system font found; raster labels disabled (set DEPWEAVE_FONT)");
        }
        TextEngine {
            font,
            cache: HashMap::new(),
        }
    }

    pub fn available(&self) -> bool {
        self.font.is_some()
    }

    /// Approximate width without rasterising (used for label collision).
    pub fn estimate_width(text: &str, px: f32) -> f32 {
        text.chars().count() as f32 * px * 0.56
    }

    pub fn width(&mut self, text: &str, px: f32) -> f32 {
        let Some(f) = &self.font else {
            return Self::estimate_width(text, px);
        };
        text.chars().map(|c| f.metrics(c, px).advance_width).sum()
    }

    /// Draw text with its left baseline at `(x, baseline)` onto RGBA8 bytes
    /// (premultiplied, opaque target assumed).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        data: &mut [u8],
        w: u32,
        h: u32,
        text: &str,
        x: f32,
        baseline: f32,
        px: f32,
        color: [u8; 3],
    ) {
        let Some(font) = &self.font else { return };
        let key_px = (px * 4.0) as u32;
        let mut pen = x;
        for c in text.chars() {
            let (m, bmp) = self
                .cache
                .entry((c, key_px))
                .or_insert_with(|| font.rasterize(c, px));
            let gx = (pen + m.xmin as f32).round() as i32;
            let gy = (baseline - m.ymin as f32 - m.height as f32).round() as i32;
            for row in 0..m.height {
                let yy = gy + row as i32;
                if yy < 0 || yy >= h as i32 {
                    continue;
                }
                for col in 0..m.width {
                    let xx = gx + col as i32;
                    if xx < 0 || xx >= w as i32 {
                        continue;
                    }
                    let cov = bmp[row * m.width + col] as u32;
                    if cov == 0 {
                        continue;
                    }
                    let o = ((yy as u32 * w + xx as u32) * 4) as usize;
                    for k in 0..3 {
                        let d = data[o + k] as i32;
                        data[o + k] = (d + (color[k] as i32 - d) * cov as i32 / 255) as u8;
                    }
                    data[o + 3] = 255;
                }
            }
            pen += m.advance_width;
        }
        if self.cache.len() > 4000 {
            self.cache.clear();
        }
    }
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}
