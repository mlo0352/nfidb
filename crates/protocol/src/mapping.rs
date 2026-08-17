use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormalizedPoint {
    pub u: f32,
    pub v: f32,
}

impl NormalizedPoint {
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            u: finite_or(self.u, 0.0).clamp(0.0, 1.0),
            v: finite_or(self.v, 0.0).clamp(0.0, 1.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetGeometry {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

impl TargetGeometry {
    #[must_use]
    pub fn map(self, point: NormalizedPoint) -> PixelPoint {
        let point = point.clamped();
        let x_span = self.width.saturating_sub(1) as f32;
        let y_span = self.height.saturating_sub(1) as f32;
        PixelPoint {
            x: self.left.saturating_add((point.u * x_span).round() as i32),
            y: self.top.saturating_add((point.v * y_span).round() as i32),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FitMode {
    Fit,
    Fill,
    OneToOne,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoContentRect {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

impl VideoContentRect {
    #[must_use]
    pub fn normalize(self, x: f32, y: f32, clamp: bool) -> Option<NormalizedPoint> {
        if self.width <= 0.0 || self.height <= 0.0 {
            return None;
        }
        let point = NormalizedPoint {
            u: (x - self.left) / self.width,
            v: (y - self.top) / self.height,
        };
        if clamp {
            Some(point.clamped())
        } else if (0.0..=1.0).contains(&point.u) && (0.0..=1.0).contains(&point.v) {
            Some(point)
        } else {
            None
        }
    }
}

#[must_use]
pub fn content_rect(
    viewport_width: f32,
    viewport_height: f32,
    source_width: f32,
    source_height: f32,
    mode: FitMode,
) -> VideoContentRect {
    if viewport_width <= 0.0 || viewport_height <= 0.0 || source_width <= 0.0 || source_height <= 0.0 {
        return VideoContentRect {
            left: 0.0,
            top: 0.0,
            width: 0.0,
            height: 0.0,
        };
    }

    let scale = match mode {
        FitMode::Fit => (viewport_width / source_width).min(viewport_height / source_height),
        FitMode::Fill => (viewport_width / source_width).max(viewport_height / source_height),
        FitMode::OneToOne => 1.0,
    };
    let width = source_width * scale;
    let height = source_height * scale;
    VideoContentRect {
        left: (viewport_width - width) * 0.5,
        top: (viewport_height - height) * 0.5,
        width,
        height,
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

#[cfg(test)]
mod tests {
    use approx::assert_abs_diff_eq;

    use super::*;

    #[test]
    fn maps_negative_origin_and_edges() {
        let target = TargetGeometry {
            left: -1920,
            top: -100,
            width: 1920,
            height: 1080,
        };
        assert_eq!(
            target.map(NormalizedPoint { u: 0.0, v: 0.0 }),
            PixelPoint { x: -1920, y: -100 }
        );
        assert_eq!(
            target.map(NormalizedPoint { u: 1.0, v: 1.0 }),
            PixelPoint { x: -1, y: 979 }
        );
    }

    #[test]
    fn calculates_horizontal_and_vertical_letterbox() {
        let horizontal = content_rect(1366.0, 1024.0, 1920.0, 1080.0, FitMode::Fit);
        assert_abs_diff_eq!(horizontal.left, 0.0, epsilon = 0.01);
        assert_abs_diff_eq!(horizontal.top, 127.8125, epsilon = 0.01);

        let vertical = content_rect(1920.0, 1080.0, 1024.0, 1366.0, FitMode::Fit);
        assert!(vertical.left > 500.0);
        assert_abs_diff_eq!(vertical.top, 0.0, epsilon = 0.01);
    }

    #[test]
    fn fill_crops_and_normalizes_against_source_pixels() {
        let rect = content_rect(1024.0, 768.0, 1920.0, 1080.0, FitMode::Fill);
        assert!(rect.left < 0.0);
        let center = rect.normalize(512.0, 384.0, false).expect("center is in content");
        assert_abs_diff_eq!(center.u, 0.5, epsilon = 0.001);
        assert_abs_diff_eq!(center.v, 0.5, epsilon = 0.001);
    }
}
