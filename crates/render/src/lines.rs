//! Fast anti-aliased line rasteriser (Xiaolin Wu style) that alpha-blends
//! straight into an RGBA8 buffer. Used for the bulk edge layer, where
//! tiny-skia's general stroker is far too slow for 10k+ edges at 60 fps.

#[inline]
fn blend(data: &mut [u8], w: i32, x: i32, y: i32, c: [u8; 3], a256: i32) {
    if a256 <= 0 {
        return;
    }
    let i = ((y * w + x) * 4) as usize;
    for k in 0..3 {
        let d = data[i + k] as i32;
        data[i + k] = (d + ((((c[k] as i32) - d) * a256) >> 8)) as u8;
    }
}

/// Draw one segment. `alpha` is 0..=1. `dash` = `(on, off)` pixel lengths.
/// `thick` draws a second, offset pass for ~2px lines.
#[allow(clippy::too_many_arguments)]
pub fn line(
    data: &mut [u8],
    w: u32,
    h: u32,
    seg: [f32; 4],
    color: [u8; 3],
    alpha: f32,
    dash: Option<(f32, f32)>,
    thick: bool,
) {
    let (w, h) = (w as i32, h as i32);
    let [x0, y0, x1, y1] = seg;
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        return;
    }
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.5 {
        return;
    }
    let steep = dy.abs() > dx.abs();
    // along = major axis, across = minor axis
    let (a0, a1, b0, b1, amax, bmax) = if steep {
        (y0, y1, x0, x1, h, w)
    } else {
        (x0, x1, y0, y1, w, h)
    };
    let da = a1 - a0;
    let db = b1 - b0;
    let slope = db / da;
    let (lo, hi) = if a0 < a1 { (a0, a1) } else { (a1, a0) };
    let start = lo.max(0.0).ceil() as i32;
    let end = hi.min((amax - 2) as f32).floor() as i32;
    if start > end {
        return;
    }
    let per_step = len / da.abs(); // euclidean distance per unit along major axis
    let cycle = dash.map(|(on, off)| on + off);
    for ai in start..=end {
        let t = ai as f32 - a0;
        let b = b0 + slope * t;
        if b < 0.0 || b >= (bmax - 2) as f32 {
            continue;
        }
        if let (Some((on, _)), Some(cy)) = (dash, cycle) {
            let dist = (t * per_step).abs();
            if dist % cy >= on {
                continue;
            }
        }
        let bi = b.floor() as i32;
        let f = b - bi as f32;
        let a256_lo = (alpha * (1.0 - f) * 256.0) as i32;
        let a256_hi = (alpha * f * 256.0) as i32;
        let put = |data: &mut [u8], ai: i32, bi: i32, a: i32| {
            if steep {
                blend(data, w, bi, ai, color, a)
            } else {
                blend(data, w, ai, bi, color, a)
            }
        };
        put(data, ai, bi, a256_lo);
        put(data, ai, bi + 1, a256_hi);
        if thick && bi + 2 < bmax {
            put(data, ai, bi + 2, a256_hi);
            put(data, ai, bi - 1 + (bi == 0) as i32, a256_lo);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(data: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * w + x) * 4) as usize;
        [data[i], data[i + 1], data[i + 2]]
    }

    #[test]
    fn draws_horizontal_and_vertical() {
        let (w, h) = (20u32, 20u32);
        let mut d = vec![0u8; (w * h * 4) as usize];
        line(
            &mut d,
            w,
            h,
            [2.0, 5.0, 15.0, 5.0],
            [255, 255, 255],
            1.0,
            None,
            false,
        );
        assert!(px(&d, w, 8, 5)[0] > 200);
        assert_eq!(px(&d, w, 8, 12)[0], 0);
        line(
            &mut d,
            w,
            h,
            [3.0, 2.0, 3.0, 17.0],
            [255, 0, 0],
            1.0,
            None,
            false,
        );
        assert!(px(&d, w, 3, 10)[0] > 200);
    }

    #[test]
    fn clipping_and_degenerate_inputs_are_safe() {
        let (w, h) = (16u32, 16u32);
        let mut d = vec![0u8; (w * h * 4) as usize];
        line(
            &mut d,
            w,
            h,
            [-1e6, -1e6, 1e6, 1e6],
            [255; 3],
            1.0,
            None,
            true,
        );
        line(
            &mut d,
            w,
            h,
            [f32::NAN, 0.0, 5.0, 5.0],
            [255; 3],
            1.0,
            None,
            false,
        );
        line(
            &mut d,
            w,
            h,
            [3.0, 3.0, 3.0, 3.0],
            [255; 3],
            1.0,
            None,
            false,
        );
        line(
            &mut d,
            w,
            h,
            [0.0, 15.0, 15.0, 0.0],
            [255; 3],
            0.5,
            Some((4.0, 3.0)),
            true,
        );
        assert!(d.iter().any(|&b| b > 0));
    }

    #[test]
    fn dash_leaves_gaps() {
        let (w, h) = (40u32, 4u32);
        let mut d = vec![0u8; (w * h * 4) as usize];
        line(
            &mut d,
            w,
            h,
            [0.0, 1.0, 38.0, 1.0],
            [255; 3],
            1.0,
            Some((4.0, 4.0)),
            false,
        );
        assert!(px(&d, w, 2, 1)[0] > 200);
        assert_eq!(px(&d, w, 6, 1)[0], 0);
    }
}
