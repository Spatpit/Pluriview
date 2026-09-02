use eframe::egui::{Pos2, Vec2};

const CAMERA_TRANSITION_MIN_SECS: f32 = 0.32;
const CAMERA_TRANSITION_MAX_SECS: f32 = 0.68;

/// One interruptible movement between two saved canvas camera positions.
/// Centers are stored in canvas coordinates so a transition remains stable
/// if the available viewport changes while it is running.
#[derive(Clone, Debug)]
pub struct CameraTransition {
    start_center: Pos2,
    target_center: Pos2,
    start_zoom: f32,
    target_zoom: f32,
    start_time: f64,
    duration: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct CameraSample {
    pub center: Pos2,
    pub zoom: f32,
    pub finished: bool,
}

impl CameraTransition {
    pub fn new(
        start_center: Pos2,
        target_center: Pos2,
        start_zoom: f32,
        target_zoom: f32,
        viewport_size: Vec2,
        start_time: f64,
    ) -> Self {
        let safe_start_zoom = start_zoom.max(0.01);
        let safe_target_zoom = target_zoom.max(0.01);
        let viewport_diagonal = viewport_size.length().max(1.0);
        let representative_zoom = (safe_start_zoom * safe_target_zoom).sqrt();
        let travel = start_center.distance(target_center) * representative_zoom;
        let travel_viewports = travel / viewport_diagonal;
        let zoom_octaves = (safe_target_zoom / safe_start_zoom).log2().abs();
        let perceptual_distance = travel_viewports.min(4.0) + zoom_octaves * 0.35;
        let duration = (CAMERA_TRANSITION_MIN_SECS + 0.13 * perceptual_distance.sqrt())
            .clamp(CAMERA_TRANSITION_MIN_SECS, CAMERA_TRANSITION_MAX_SECS);

        Self {
            start_center,
            target_center,
            start_zoom: safe_start_zoom,
            target_zoom: safe_target_zoom,
            start_time,
            duration,
        }
    }

    pub fn sample(&self, now: f64) -> CameraSample {
        let linear_t = (((now - self.start_time) as f32) / self.duration).clamp(0.0, 1.0);
        // Fifth-order smootherstep starts and ends with zero velocity and
        // acceleration, avoiding a visible snap at either end of the move.
        let t = linear_t * linear_t * linear_t * (linear_t * (linear_t * 6.0 - 15.0) + 10.0);
        let center = self.start_center + (self.target_center - self.start_center) * t;
        // Zoom is perceived as a ratio. Interpolating in log space makes
        // zooming in and out feel equally paced.
        let zoom =
            (self.start_zoom.ln() + (self.target_zoom.ln() - self.start_zoom.ln()) * t).exp();

        CameraSample {
            center,
            zoom,
            finished: linear_t >= 1.0,
        }
    }
}

/// Tracks drag velocity for canvas momentum scrolling.
#[derive(Clone, Debug)]
pub struct DragTracker {
    /// History of positions for velocity calculation
    positions: Vec<(Pos2, f64)>, // (position, time)
    /// Maximum number of samples to keep
    max_samples: usize,
}

impl DragTracker {
    pub fn new() -> Self {
        Self {
            positions: Vec::with_capacity(5),
            max_samples: 5,
        }
    }

    /// Record a position sample
    pub fn record(&mut self, pos: Pos2, time: f64) {
        self.positions.push((pos, time));
        if self.positions.len() > self.max_samples {
            self.positions.remove(0);
        }
    }

    /// Calculate average velocity from recent samples (pixels per second)
    pub fn get_velocity(&self) -> Vec2 {
        if self.positions.len() < 2 {
            return Vec2::ZERO;
        }

        // Use weighted average of recent velocities (more recent = more weight)
        let mut total_vel = Vec2::ZERO;
        let mut total_weight = 0.0;

        for i in 1..self.positions.len() {
            let (pos1, t1) = self.positions[i - 1];
            let (pos2, t2) = self.positions[i];

            let dt = (t2 - t1) as f32;
            if dt > 0.001 {
                let vel = (pos2 - pos1) / dt;
                let weight = (i as f32) / (self.positions.len() as f32); // More recent = higher weight
                total_vel += vel * weight;
                total_weight += weight;
            }
        }

        if total_weight > 0.0 {
            total_vel / total_weight
        } else {
            Vec2::ZERO
        }
    }

    /// Clear all samples
    pub fn clear(&mut self) {
        self.positions.clear();
    }
}

impl Default for DragTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Animation state for the canvas
#[derive(Clone, Debug, Default)]
pub struct AnimationState {
    /// Is momentum animation active?
    pub momentum_active: bool,

    /// Current momentum velocity (for pan)
    pub momentum_velocity: Vec2,

    /// Saved-view camera motion, kept separate from canvas pan momentum so it
    /// can be retargeted or interrupted cleanly.
    pub camera_transition: Option<CameraTransition>,
}

impl AnimationState {
    pub fn new() -> Self {
        Self {
            momentum_active: false,
            momentum_velocity: Vec2::ZERO,
            camera_transition: None,
        }
    }

    /// Update all animations (call each frame)
    pub fn update(&mut self) {
        // Apply momentum with friction
        if self.momentum_active {
            let friction = 0.85; // Stronger friction = faster stop
            self.momentum_velocity *= friction;

            // Stop momentum when slow enough
            if self.momentum_velocity.length() < 0.3 {
                self.momentum_velocity = Vec2::ZERO;
                self.momentum_active = false;
            }
        }
    }

    /// Check if any animations are currently running
    pub fn is_animating(&self) -> bool {
        self.camera_transition.is_some() || self.momentum_active
    }

    /// Start momentum with given velocity
    pub fn start_momentum(&mut self, velocity: Vec2) {
        // Scale down velocity for subtle momentum
        self.momentum_velocity = velocity * 0.008; // Much less momentum
        self.momentum_active = self.momentum_velocity.length() > 0.5;
    }

    /// Get current momentum delta (apply this to pan each frame)
    pub fn get_momentum_delta(&self) -> Vec2 {
        self.momentum_velocity
    }
}

#[cfg(test)]
mod tests {
    use super::CameraTransition;
    use eframe::egui::{Pos2, Vec2};

    #[test]
    fn camera_transition_lands_exactly_on_its_target() {
        let transition = CameraTransition::new(
            Pos2::new(-40.0, 10.0),
            Pos2::new(900.0, -250.0),
            0.5,
            2.0,
            Vec2::new(1280.0, 720.0),
            5.0,
        );

        let start = transition.sample(5.0);
        assert_eq!(start.center, Pos2::new(-40.0, 10.0));
        assert!((start.zoom - 0.5).abs() < f32::EPSILON);
        assert!(!start.finished);

        let end = transition.sample(10.0);
        assert_eq!(end.center, Pos2::new(900.0, -250.0));
        assert!((end.zoom - 2.0).abs() < 0.0001);
        assert!(end.finished);
    }

    #[test]
    fn zoom_midpoint_uses_equal_ratios() {
        let transition = CameraTransition::new(
            Pos2::ZERO,
            Pos2::ZERO,
            0.5,
            2.0,
            Vec2::new(1280.0, 720.0),
            0.0,
        );
        let midpoint = transition.sample(f64::from(transition.duration) * 0.5);

        assert!((midpoint.zoom - 1.0).abs() < 0.0001);
    }
}
