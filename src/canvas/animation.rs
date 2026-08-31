use crate::preview::PreviewId;
use eframe::egui::{Pos2, Vec2};
use std::collections::HashMap;

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

/// A single spring-animated value with smooth easing
#[derive(Clone, Debug)]
pub struct SpringValue {
    /// Current animated value
    pub current: f32,
    /// Target value to animate towards
    pub target: f32,
    /// Current velocity
    pub velocity: f32,
    /// Spring stiffness (0.0-1.0, higher = faster response)
    pub stiffness: f32,
    /// Damping factor (0.0-1.0, higher = less bouncy)
    pub damping: f32,
}

impl SpringValue {
    pub fn new(initial: f32) -> Self {
        Self {
            current: initial,
            target: initial,
            velocity: 0.0,
            stiffness: 0.08, // Very smooth, subtle movement
            damping: 0.65,   // Heavy damping, almost no bounce
        }
    }

    /// Update the spring animation (call each frame)
    /// Note: dt is passed for API consistency but animation uses fixed timestep
    pub fn update(&mut self, _dt: f32) {
        // Spring force calculation
        let displacement = self.target - self.current;

        // Spring acceleration: a = stiffness * displacement
        let spring_force = self.stiffness * displacement;

        // Apply spring force and damping
        self.velocity += spring_force;
        self.velocity *= self.damping;

        // Update position
        self.current += self.velocity;

        // Snap to target when close enough (prevents infinite tiny oscillations)
        if displacement.abs() < 0.5 && self.velocity.abs() < 0.1 {
            self.current = self.target;
            self.velocity = 0.0;
        }
    }

    /// Set a new target value
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Jump immediately to a value (no animation)
    pub fn set_immediate(&mut self, value: f32) {
        self.current = value;
        self.target = value;
        self.velocity = 0.0;
    }

    /// Check if currently animating
    pub fn is_animating(&self) -> bool {
        (self.target - self.current).abs() > 0.5 || self.velocity.abs() > 0.1
    }

    /// Add velocity (for momentum)
    pub fn add_velocity(&mut self, vel: f32) {
        self.velocity += vel;
    }
}

/// Spring-animated 2D position
#[derive(Clone, Debug)]
pub struct SpringVec2 {
    pub x: SpringValue,
    pub y: SpringValue,
}

impl SpringVec2 {
    pub fn new(initial: Vec2) -> Self {
        Self {
            x: SpringValue::new(initial.x),
            y: SpringValue::new(initial.y),
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.x.update(dt);
        self.y.update(dt);
    }

    pub fn current_pos(&self) -> Pos2 {
        Pos2::new(self.x.current, self.y.current)
    }

    pub fn set_target_pos(&mut self, target: Pos2) {
        self.x.set_target(target.x);
        self.y.set_target(target.y);
    }

    pub fn set_immediate_pos(&mut self, value: Pos2) {
        self.x.set_immediate(value.x);
        self.y.set_immediate(value.y);
    }

    pub fn is_animating(&self) -> bool {
        self.x.is_animating() || self.y.is_animating()
    }

    pub fn add_velocity(&mut self, vel: Vec2) {
        self.x.add_velocity(vel.x);
        self.y.add_velocity(vel.y);
    }
}

/// Tracks drag velocity for momentum scrolling
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

/// Snap-to-grid configuration
#[derive(Clone, Debug)]
pub struct SnapConfig {
    /// Is snap-to-grid enabled?
    pub enabled: bool,
    /// Grid cell size
    pub grid_size: f32,
    /// Distance threshold for snapping (in canvas units)
    pub snap_threshold: f32,
}

impl Default for SnapConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            grid_size: 50.0,
            snap_threshold: 15.0, // Weaker snap - only very close to grid
        }
    }
}

impl SnapConfig {
    /// Get the snapped position if within threshold, otherwise return original
    pub fn snap_position(&self, pos: Pos2) -> Pos2 {
        if !self.enabled {
            return pos;
        }

        let snapped_x = (pos.x / self.grid_size).round() * self.grid_size;
        let snapped_y = (pos.y / self.grid_size).round() * self.grid_size;
        let snapped = Pos2::new(snapped_x, snapped_y);

        // Only snap if within threshold
        let dist = (pos - snapped).length();
        if dist <= self.snap_threshold {
            snapped
        } else {
            pos
        }
    }
}

/// Animation state for the canvas
#[derive(Clone, Debug, Default)]
pub struct AnimationState {
    /// Spring animations for each preview's position
    pub preview_springs: HashMap<PreviewId, SpringVec2>,

    /// Drag velocity tracker (for momentum)
    pub drag_tracker: DragTracker,

    /// Is momentum animation active?
    pub momentum_active: bool,

    /// Current momentum velocity (for pan)
    pub momentum_velocity: Vec2,

    /// Snap-to-grid configuration
    pub snap_config: SnapConfig,

    /// Last frame time for delta calculation
    pub last_frame_time: f64,

    /// Saved-view camera motion, kept separate from drag momentum so it can
    /// be retargeted or interrupted without disturbing tile animations.
    pub camera_transition: Option<CameraTransition>,
}

impl AnimationState {
    pub fn new() -> Self {
        Self {
            preview_springs: HashMap::new(),
            drag_tracker: DragTracker::new(),
            momentum_active: false,
            momentum_velocity: Vec2::ZERO,
            snap_config: SnapConfig::default(),
            last_frame_time: 0.0,
            camera_transition: None,
        }
    }

    /// Get or create a spring for a preview
    pub fn get_or_create_spring(&mut self, id: PreviewId, initial_pos: Pos2) -> &mut SpringVec2 {
        self.preview_springs
            .entry(id)
            .or_insert_with(|| SpringVec2::new(initial_pos.to_vec2()))
    }

    /// Update all animations (call each frame)
    pub fn update(&mut self, dt: f32) {
        // Update preview springs
        for spring in self.preview_springs.values_mut() {
            spring.update(dt);
        }

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
        self.camera_transition.is_some()
            || self.momentum_active
            || self.preview_springs.values().any(|s| s.is_animating())
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
