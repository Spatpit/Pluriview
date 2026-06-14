use eframe::egui::{self, Pos2, Vec2, Rounding, Stroke, RichText};
use super::{WindowInfo, enumerate_windows};
use crate::preview::PreviewManager;
use crate::capture::CaptureCoordinator;
use crate::canvas::CanvasState;

/// Window picker panel state
pub struct WindowPicker {
    /// Cached list of windows
    windows: Vec<WindowInfo>,

    /// Search filter text
    search_filter: String,

    /// Last refresh time
    last_refresh: std::time::Instant,

    /// Auto-refresh interval
    refresh_interval: std::time::Duration,
}

impl WindowPicker {
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            search_filter: String::new(),
            last_refresh: std::time::Instant::now() - std::time::Duration::from_secs(10),
            refresh_interval: std::time::Duration::from_secs(2),
        }
    }

    /// Refresh the window list
    pub fn refresh(&mut self) {
        self.windows = enumerate_windows();
        self.last_refresh = std::time::Instant::now();
    }

    /// UI for the window picker
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
        canvas: &CanvasState,
    ) {
        // Auto-refresh
        if self.last_refresh.elapsed() > self.refresh_interval {
            self.refresh();
        }

        // Colors for the modern theme
        let card_bg = egui::Color32::from_rgb(28, 28, 32);
        let card_hover = egui::Color32::from_rgb(38, 38, 45);
        let accent_color = egui::Color32::from_rgb(107, 170, 75); // Leaf green
        let text_secondary = egui::Color32::from_rgb(140, 140, 150);
        let search_bg = egui::Color32::from_rgb(22, 22, 26);

        ui.add_space(4.0);

        // Modern search box with rounded corners
        let search_frame = egui::Frame::none()
            .fill(search_bg)
            .rounding(Rounding::same(8.0))
            .inner_margin(egui::Margin::symmetric(12.0, 8.0));

        search_frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                // Search icon (magnifying glass)
                ui.label(RichText::new("🔍").size(14.0).color(text_secondary));
                ui.add_space(6.0);

                // Search input with placeholder
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.search_filter)
                        .desired_width(ui.available_width())
                        .hint_text(RichText::new("Search windows...").color(text_secondary))
                        .frame(false)
                );

                // Escape clears search
                if !self.search_filter.is_empty() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.search_filter.clear();
                    response.request_focus();
                }
            });
        });

        ui.add_space(8.0);

        // Window count and refresh indicator
        ui.horizontal(|ui| {
            let filter_lower = self.search_filter.to_lowercase();
            let count = self.windows.iter()
                .filter(|w| Self::window_matches(w, &filter_lower))
                .count();

            ui.label(
                RichText::new(format!("{} windows", count))
                    .size(12.0)
                    .color(text_secondary)
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Refresh button (subtle, icon-based)
                let refresh_btn = ui.add(
                    egui::Button::new(RichText::new("↻").size(14.0))
                        .frame(false)
                );
                if refresh_btn.clicked() {
                    self.refresh();
                }
                if refresh_btn.hovered() {
                    egui::show_tooltip(ui.ctx(), ui.layer_id(), egui::Id::new("refresh_tooltip"), |ui| {
                        ui.label("Refresh window list");
                    });
                }
            });
        });

        ui.add_space(6.0);

        // Precompute the filtered set once (avoids cloning the whole window
        // list and re-filtering it several times every frame)
        let filter_lower = self.search_filter.to_lowercase();
        let filtered: Vec<usize> = self.windows.iter()
            .enumerate()
            .filter(|(_, w)| Self::window_matches(w, &filter_lower))
            .map(|(i, _)| i)
            .collect();

        // Window list with card-style items
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let available_width = ui.available_width();

                for &idx in &filtered {
                    let window = &self.windows[idx];

                    // Card frame
                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(available_width, 56.0),
                        egui::Sense::click()
                    );

                    let is_hovered = response.hovered();
                    let bg_color = if is_hovered { card_hover } else { card_bg };

                    // Draw card background
                    ui.painter().rect_filled(
                        rect,
                        Rounding::same(6.0),
                        bg_color
                    );

                    // Draw subtle border on hover
                    if is_hovered {
                        ui.painter().rect_stroke(
                            rect,
                            Rounding::same(6.0),
                            Stroke::new(1.0, egui::Color32::from_rgb(50, 50, 58))
                        );
                    }

                    // Content layout
                    let inner_rect = rect.shrink(10.0);
                    let text_rect = egui::Rect::from_min_max(
                        inner_rect.min,
                        egui::Pos2::new(inner_rect.max.x - 36.0, inner_rect.max.y)
                    );
                    let button_rect = egui::Rect::from_min_max(
                        egui::Pos2::new(inner_rect.max.x - 30.0, inner_rect.min.y + 8.0),
                        egui::Pos2::new(inner_rect.max.x, inner_rect.max.y - 8.0)
                    );

                    // Title (truncated, char-safe to avoid panics on multibyte titles)
                    let max_title_chars = ((text_rect.width() - 10.0) / 7.0) as usize;
                    let title = if window.title.chars().count() > max_title_chars {
                        let kept: String = window.title.chars()
                            .take(max_title_chars.saturating_sub(3))
                            .collect();
                        format!("{}...", kept)
                    } else {
                        window.title.clone()
                    };

                    // Draw title
                    ui.painter().text(
                        egui::Pos2::new(text_rect.min.x, text_rect.min.y + 2.0),
                        egui::Align2::LEFT_TOP,
                        &title,
                        egui::FontId::proportional(14.0),
                        egui::Color32::WHITE
                    );

                    // Draw exe name
                    ui.painter().text(
                        egui::Pos2::new(text_rect.min.x, text_rect.min.y + 20.0),
                        egui::Align2::LEFT_TOP,
                        &window.exe_name,
                        egui::FontId::proportional(11.0),
                        text_secondary
                    );

                    // Add button (+ icon)
                    let btn_center = button_rect.center();
                    let btn_radius = 14.0;
                    let btn_rect = egui::Rect::from_center_size(btn_center, Vec2::splat(btn_radius * 2.0));

                    let btn_response = ui.interact(btn_rect, response.id.with("add_btn"), egui::Sense::click());
                    let btn_hovered = btn_response.hovered();

                    // Draw + button circle
                    ui.painter().circle_filled(
                        btn_center,
                        btn_radius,
                        if btn_hovered { accent_color } else { egui::Color32::from_rgb(60, 60, 68) }
                    );

                    // Draw + icon
                    let plus_color = if btn_hovered { egui::Color32::WHITE } else { egui::Color32::from_rgb(180, 180, 190) };
                    ui.painter().text(
                        btn_center,
                        egui::Align2::CENTER_CENTER,
                        "+",
                        egui::FontId::proportional(18.0),
                        plus_color
                    );

                    // Handle add button click
                    if btn_response.clicked() {
                        self.add_window_to_canvas(
                            window,
                            preview_manager,
                            capture_coordinator,
                            canvas
                        );
                    }

                    ui.add_space(4.0);
                }

                // Empty state
                if filtered.is_empty() && !filter_lower.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("No matching windows")
                                .size(13.0)
                                .color(text_secondary)
                        );
                    });
                }
            });
    }

    /// Returns true if a window matches the (already lowercased) search filter.
    /// An empty filter matches everything.
    fn window_matches(w: &WindowInfo, filter_lower: &str) -> bool {
        filter_lower.is_empty()
            || w.title.to_lowercase().contains(filter_lower)
            || w.exe_name.to_lowercase().contains(filter_lower)
    }

    /// Add a window to the canvas
    fn add_window_to_canvas(
        &self,
        window: &WindowInfo,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
        canvas: &CanvasState,
    ) {
        // Calculate position (center of current viewport with offset)
        let preview_count = preview_manager.count();
        let offset = Vec2::new(
            (preview_count % 3) as f32 * 50.0,
            (preview_count / 3) as f32 * 50.0,
        );

        let position = Pos2::new(
            -canvas.pan.x + 50.0 + offset.x,
            -canvas.pan.y + 50.0 + offset.y,
        );

        // Default preview size
        let size = Vec2::new(320.0, 240.0);

        // Add preview
        let id = preview_manager.add_for_window(
            window.hwnd,
            window.process_id,
            window.title.clone(),
            position,
            size,
        );

        // Mark as capturing
        if let Some(preview) = preview_manager.get_mut(id) {
            preview.capture_active = true;
        }

        // Start capture with window title for matching
        capture_coordinator.start_capture(id, window.hwnd, window.title.clone(), 30);
    }
}

impl Default for WindowPicker {
    fn default() -> Self {
        Self::new()
    }
}
