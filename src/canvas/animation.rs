use eframe::egui::{Pos2, Vec2};
use std::collections::HashMap;
use crate::preview::PreviewId;

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
            stiffness: 0.08,  // Very smooth, subtle movement
            damping: 0.65,    // Heavy damping, almost no bounce
        }
    }

    #[allow(dead_code)]
    pub fn with_params(initial: f32, stiffness: f32, damping: f32) -> Self {
        Self {
            current: initial,
            target: initial,
            velocity: 0.0,
            stiffness,
            damping,
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

    #[allow(dead_code)]
    pub fn with_params(initial: Vec2, stiffness: f32, damping: f32) -> Self {
        Self {
            x: SpringValue::with_params(initial.x, stiffness, damping),
            y: SpringValue::with_params(initial.y, stiffness, damping),
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.x.update(dt);
        self.y.update(dt);
    }

    #[allow(dead_code)]
    pub fn current(&self) -> Vec2 {
        Vec2::new(self.x.current, self.y.current)
    }

    pub fn current_pos(&self) -> Pos2 {
        Pos2::new(self.x.current, self.y.current)
    }

    #[allow(dead_code)]
    pub fn set_target(&mut self, target: Vec2) {
        self.x.set_target(target.x);
        self.y.set_target(target.y);
    }

    pub fn set_target_pos(&mut self, target: Pos2) {
        self.x.set_target(target.x);
        self.y.set_target(target.y);
    }

    #[allow(dead_code)]
    pub fn set_immediate(&mut self, value: Vec2) {
        self.x.set_immediate(value.x);
        self.y.set_immediate(value.y);
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
    positions: Vec<(Pos2, f64)>,  // (position, time)
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
            snap_threshold: 15.0,  // Weaker snap - only very close to grid
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

    /// Always snap to nearest grid position
    #[allow(dead_code)]
    pub fn force_snap(&self, pos: Pos2) -> Pos2 {
        let snapped_x = (pos.x / self.grid_size).round() * self.grid_size;
        let snapped_y = (pos.y / self.grid_size).round() * self.grid_size;
        Pos2::new(snapped_x, snapped_y)
    }
}

/// Animation state for the canvas
#[derive(Clone, Debug, Default)]
pub struct AnimationState {
    /// Spring animations for each preview's position
    pub preview_springs: HashMap<PreviewId, SpringVec2>,

    /// Spring animation for canvas pan
    pub pan_spring: Option<SpringVec2>,

    /// Spring animation for canvas zoom
    pub zoom_spring: Option<SpringValue>,

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
}

impl AnimationState {
    pub fn new() -> Self {
        Self {
            preview_springs: HashMap::new(),
            pan_spring: None,
            zoom_spring: None,
            drag_tracker: DragTracker::new(),
            momentum_active: false,
            momentum_velocity: Vec2::ZERO,
            snap_config: SnapConfig::default(),
            last_frame_time: 0.0,
        }
    }

    /// Get or create a spring for a preview
    pub fn get_or_create_spring(&mut self, id: PreviewId, initial_pos: Pos2) -> &mut SpringVec2 {
        self.preview_springs.entry(id).or_insert_with(|| {
            SpringVec2::new(initial_pos.to_vec2())
        })
    }

    /// Remove spring for a preview (when preview is deleted)
    #[allow(dead_code)]
    pub fn remove_spring(&mut self, id: PreviewId) {
        self.preview_springs.remove(&id);
    }

    /// Update all animations (call each frame)
    pub fn update(&mut self, dt: f32) {
        // Update preview springs
        for spring in self.preview_springs.values_mut() {
            spring.update(dt);
        }

        // Update pan spring
        if let Some(ref mut pan) = self.pan_spring {
            pan.update(dt);
        }

        // Update zoom spring
        if let Some(ref mut zoom) = self.zoom_spring {
            zoom.update(dt);
        }

        // Apply momentum with friction
        if self.momentum_active {
            let friction = 0.85;  // Stronger friction = faster stop
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
        self.momentum_active
            || self.preview_springs.values().any(|s| s.is_animating())
            || self.pan_spring.as_ref().map(|s| s.is_animating()).unwrap_or(false)
            || self.zoom_spring.as_ref().map(|s| s.is_animating()).unwrap_or(false)
    }

    /// Start momentum with given velocity
    pub fn start_momentum(&mut self, velocity: Vec2) {
        // Scale down velocity for subtle momentum
        self.momentum_velocity = velocity * 0.008;  // Much less momentum
        self.momentum_active = self.momentum_velocity.length() > 0.5;
    }

    /// Get current momentum delta (apply this to pan each frame)
    pub fn get_momentum_delta(&self) -> Vec2 {
        self.momentum_velocity
    }
}
