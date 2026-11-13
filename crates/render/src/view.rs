//! Pan/zoom camera.

pub const MIN_ZOOM: f32 = 0.02;
pub const MAX_ZOOM: f32 = 40.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// World point at the centre of the viewport.
    pub cx: f32,
    pub cy: f32,
    /// Screen pixels per world unit.
    pub zoom: f32,
    /// Viewport size in pixels.
    pub width: f32,
    pub height: f32,
}

impl Default for View {
    fn default() -> Self {
        View {
            cx: 0.0,
            cy: 0.0,
            zoom: 1.0,
            width: 800.0,
            height: 600.0,
        }
    }
}

impl View {
    pub fn to_screen(&self, p: [f32; 2]) -> [f32; 2] {
        [
            (p[0] - self.cx) * self.zoom + self.width / 2.0,
            (p[1] - self.cy) * self.zoom + self.height / 2.0,
        ]
    }

    pub fn to_world(&self, s: [f32; 2]) -> [f32; 2] {
        [
            (s[0] - self.width / 2.0) / self.zoom + self.cx,
            (s[1] - self.height / 2.0) / self.zoom + self.cy,
        ]
    }

    /// Zoom by `factor`, keeping the world point under screen point `s` fixed.
    pub fn zoom_at(&mut self, s: [f32; 2], factor: f32) {
        let before = self.to_world(s);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let after = self.to_world(s);
        self.cx += before[0] - after[0];
        self.cy += before[1] - after[1];
    }

    /// Pan by a screen-space delta (content follows the pointer).
    pub fn pan(&mut self, dx: f32, dy: f32) {
        self.cx -= dx / self.zoom;
        self.cy -= dy / self.zoom;
    }

    /// World-space rectangle `(minx, miny, maxx, maxy)` that is visible.
    pub fn world_rect(&self, margin_px: f32) -> (f32, f32, f32, f32) {
        let hw = (self.width / 2.0 + margin_px) / self.zoom;
        let hh = (self.height / 2.0 + margin_px) / self.zoom;
        (self.cx - hw, self.cy - hh, self.cx + hw, self.cy + hh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_zoom_anchor() {
        let mut v = View {
            cx: 10.0,
            cy: -5.0,
            zoom: 2.0,
            width: 400.0,
            height: 300.0,
        };
        let w = [33.0, 7.0];
        let s = v.to_screen(w);
        let back = v.to_world(s);
        assert!((back[0] - w[0]).abs() < 1e-3 && (back[1] - w[1]).abs() < 1e-3);
        let anchor = [100.0, 80.0];
        let before = v.to_world(anchor);
        v.zoom_at(anchor, 1.7);
        let after = v.to_world(anchor);
        assert!((before[0] - after[0]).abs() < 1e-3 && (before[1] - after[1]).abs() < 1e-3);
        v.zoom_at(anchor, 1e9);
        assert_eq!(v.zoom, MAX_ZOOM);
    }
}
