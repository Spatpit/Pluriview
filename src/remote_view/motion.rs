use super::CameraCommand;
use crate::canvas::CanvasState;
use eframe::egui::{Rect, Vec2};

/// A short presentation buffer for uneven network arrivals. Commands compose
/// against the complete target, so smoothing never loses pinch distance.
pub struct Motion {
    target_pan: Vec2,
    target_zoom: f32,
    last_pan: Vec2,
    last_zoom: f32,
    rect: Rect,
    updated: f64,
    stepped: f64,
}

impl Motion {
    pub fn new(canvas: &CanvasState, now: f64) -> Option<Self> {
        Some(Self {
            target_pan: canvas.pan,
            target_zoom: canvas.zoom,
            last_pan: canvas.pan,
            last_zoom: canvas.zoom,
            rect: canvas.last_screen_rect?,
            updated: now,
            stepped: now,
        })
    }

    pub fn matches(&self, canvas: &CanvasState) -> bool {
        canvas.pan == self.last_pan
            && canvas.zoom == self.last_zoom
            && canvas.last_screen_rect == Some(self.rect)
    }

    pub fn push(&mut self, canvas: &mut CanvasState, command: CameraCommand, now: f64) {
        let displayed = (canvas.pan, canvas.zoom);
        canvas.pan = self.target_pan;
        canvas.zoom = self.target_zoom;
        canvas.apply_viewer_camera(command);
        self.target_pan = canvas.pan;
        self.target_zoom = canvas.zoom;
        (canvas.pan, canvas.zoom) = displayed;
        self.updated = now;
    }

    /// Interpolate the screen transform, preserving a stationary pinch anchor.
    /// Return false after catching up exactly, so an idle viewer stops repainting.
    pub fn advance(&mut self, canvas: &mut CanvasState, now: f64) -> bool {
        let finished = now - self.updated >= 0.100;
        let amount = if finished {
            1.0
        } else {
            (1.0 - (-(now - self.stepped).max(0.0) / 0.025).exp()) as f32
        };
        let translation = canvas.pan * canvas.zoom;
        canvas.zoom = if finished {
            self.target_zoom
        } else {
            canvas.zoom + (self.target_zoom - canvas.zoom) * amount
        };
        canvas.pan = if finished {
            self.target_pan
        } else {
            (translation + (self.target_pan * self.target_zoom - translation) * amount)
                / canvas.zoom
        };
        self.last_pan = canvas.pan;
        self.last_zoom = canvas.zoom;
        self.stepped = now;
        !finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, vec2};

    #[test]
    fn buffered_pinch_preserves_anchor_and_all_accumulated_movement() {
        let mut canvas = CanvasState::default();
        let rect = Rect::from_min_size(pos2(20.0, 30.0), vec2(800.0, 600.0));
        canvas.last_screen_rect = Some(rect);
        let mut expected = CanvasState::default();
        expected.last_screen_rect = Some(rect);
        let mut motion = Motion::new(&canvas, 0.0).unwrap();
        let command = CameraCommand {
            pan: [0.0; 2],
            scale: 1.4,
            anchor: [0.3, 0.7],
            viewport: [800.0, 600.0],
        };
        let anchor = rect.min + rect.size() * vec2(0.3, 0.7);
        let point = canvas.screen_to_canvas(anchor, rect);
        for i in 0..4 {
            let now = i as f64 * 0.02;
            motion.push(&mut canvas, command, now);
            expected.apply_viewer_camera(command);
            assert!(motion.advance(&mut canvas, now + 0.01));
            assert!(canvas.canvas_to_screen(point, rect).distance(anchor) < 0.001);
            assert!(canvas.zoom < expected.zoom);
        }
        assert!(!motion.advance(&mut canvas, 0.2));
        assert!((canvas.pan - expected.pan).length() < 0.001);
        assert_eq!(canvas.zoom, expected.zoom);
        assert!(motion.matches(&canvas));
        canvas.pan.x += 1.0;
        assert!(
            !motion.matches(&canvas),
            "Local navigation must cancel smoothing"
        );
    }

    #[test]
    fn changing_anchors_pan_and_zoom_limits_compose_against_the_target() {
        let mut canvas = CanvasState::default();
        canvas.last_screen_rect = Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)));
        let mut expected = CanvasState::default();
        expected.last_screen_rect = canvas.last_screen_rect;
        let mut motion = Motion::new(&canvas, 0.0).unwrap();
        for (i, (scale, anchor, pan)) in [
            (4.0, [0.2, 0.8], [0.1, -0.2]),
            (4.0, [0.8, 0.2], [-0.1, 0.1]),
            (0.25, [0.5, 0.5], [0.0, 0.1]),
        ]
        .into_iter()
        .enumerate()
        {
            let c = CameraCommand {
                scale,
                anchor,
                pan,
                viewport: [800.0, 600.0],
            };
            motion.push(&mut canvas, c, i as f64 * 0.02);
            motion.advance(&mut canvas, i as f64 * 0.02 + 0.01);
            expected.apply_viewer_camera(c);
        }
        assert!(!motion.advance(&mut canvas, 0.3));
        assert_eq!(canvas.zoom, expected.zoom);
        assert!((canvas.pan - expected.pan).length() < 0.001);
    }
}
