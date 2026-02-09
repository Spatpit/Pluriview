use eframe::egui::{self, Pos2, Vec2};
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

        ui.heading("Window Picker");
        ui.separator();

        // Search box
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut self.search_filter);
        });

        // Refresh button
        ui.horizontal(|ui| {
            if ui.button("Refresh").clicked() {
                self.refresh();
            }
            ui.label(format!("{} windows", self.windows.len()));
        });

        ui.separator();

        // Window list
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let filter_lower = self.search_filter.to_lowercase();

                for window in &self.windows {
                    // Apply filter
                    if !filter_lower.is_empty() {
                        let title_lower = window.title.to_lowercase();
                        let exe_lower = window.exe_name.to_lowercase();
                        if !title_lower.contains(&filter_lower) && !exe_lower.contains(&filter_lower) {
                            continue;
                        }
                    }

                    ui.horizontal(|ui| {
                        // Add button
                        if ui.button("+").on_hover_text("Add to canvas").clicked() {
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

                        // Window info
                        ui.vertical(|ui| {
                            // Title (truncated)
                            let title = if window.title.len() > 35 {
                                format!("{}...", &window.title[..32])
                            } else {
                                window.title.clone()
                            };

                            ui.label(title);

                            // Executable name
                            ui.colored_label(
                                egui::Color32::GRAY,
                                &window.exe_name,
                            );
                        });
                    });

                    ui.separator();
                }
            });
    }
}

impl Default for WindowPicker {
    fn default() -> Self {
        Self::new()
    }
}
