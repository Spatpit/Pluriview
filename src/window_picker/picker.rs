use super::{enumerate_windows, WindowInfo};
use crate::canvas::CanvasState;
use crate::capture::CaptureCoordinator;
use crate::preview::{FpsPreset, PreviewManager};
use crate::spout::{self, SpoutDetection, SpoutSender};
use eframe::egui::{self, Pos2, RichText, Rounding, Stroke, Vec2};
use std::borrow::Cow;

#[derive(Clone, Copy)]
struct PickerPalette {
    text_secondary: egui::Color32,
    card_bg: egui::Color32,
    card_hover: egui::Color32,
}

/// Window picker panel state
pub struct WindowPicker {
    /// Cached list of windows
    windows: Vec<WindowInfo>,

    /// Latest Spout2 sender snapshot
    spout: SpoutDetection,

    /// Search filter text
    search_filter: String,

    /// Last refresh time
    last_refresh: std::time::Instant,

    /// Auto-refresh interval
    refresh_interval: std::time::Duration,

    /// Cached normalized filter and matching window indices. Rebuilt only
    /// when the query or enumerated window list changes.
    normalized_filter: String,
    filtered_indices: Vec<usize>,
    filtered_spout_indices: Vec<usize>,
    filter_dirty: bool,
}

impl WindowPicker {
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            spout: SpoutDetection::default(),
            search_filter: String::new(),
            last_refresh: std::time::Instant::now() - std::time::Duration::from_secs(10),
            refresh_interval: std::time::Duration::from_secs(2),
            normalized_filter: String::new(),
            filtered_indices: Vec::new(),
            filtered_spout_indices: Vec::new(),
            filter_dirty: true,
        }
    }

    /// Refresh the window list and Spout2 sender snapshot
    pub fn refresh(&mut self) {
        self.windows = enumerate_windows();
        self.spout = spout::detect();
        self.last_refresh = std::time::Instant::now();
        self.filter_dirty = true;
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
                ui.label(
                    RichText::new(egui_phosphor::regular::MAGNIFYING_GLASS)
                        .size(14.0)
                        .color(text_secondary),
                );
                ui.add_space(6.0);

                // Search input with placeholder
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.search_filter)
                        .desired_width(ui.available_width())
                        .hint_text(
                            RichText::new("Search windows and Spout...").color(text_secondary),
                        )
                        .frame(false),
                );
                if response.changed() {
                    self.filter_dirty = true;
                }

                // Escape clears search
                if !self.search_filter.is_empty() && ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    self.search_filter.clear();
                    self.filter_dirty = true;
                    response.request_focus();
                }
            });
        });

        ui.add_space(8.0);
        self.update_filter();
        self.spout_section(
            ui,
            preview_manager,
            capture_coordinator,
            canvas,
            PickerPalette {
                text_secondary,
                card_bg,
                card_hover,
            },
        );

        // Window count and refresh indicator
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} windows", self.filtered_indices.len()))
                    .size(12.0)
                    .color(text_secondary),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Refresh button (subtle, icon-based)
                let refresh_btn = ui.add(
                    egui::Button::new(
                        RichText::new(egui_phosphor::regular::ARROW_CLOCKWISE).size(14.0),
                    )
                    .frame(false),
                );
                if refresh_btn.clicked() {
                    self.refresh();
                }
                if refresh_btn.hovered() {
                    egui::show_tooltip(
                        ui.ctx(),
                        ui.layer_id(),
                        egui::Id::new("refresh_tooltip"),
                        |ui| {
                            ui.label("Refresh window list");
                        },
                    );
                }
            });
        });

        ui.add_space(6.0);
        self.update_filter();

        // Window list with fixed-height row virtualization, so only visible
        // cards allocate egui widgets and text each frame.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, 60.0, self.filtered_indices.len(), |ui, row_range| {
                let available_width = ui.available_width();

                for row in row_range {
                    let idx = self.filtered_indices[row];
                    let window = &self.windows[idx];

                    // Card frame
                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(available_width, 56.0),
                        egui::Sense::click(),
                    );

                    let is_hovered = response.hovered();
                    let bg_color = if is_hovered { card_hover } else { card_bg };

                    // Draw card background
                    ui.painter()
                        .rect_filled(rect, Rounding::same(6.0), bg_color);

                    // Draw subtle border on hover
                    if is_hovered {
                        ui.painter().rect_stroke(
                            rect,
                            Rounding::same(6.0),
                            Stroke::new(1.0, egui::Color32::from_rgb(50, 50, 58)),
                        );
                    }

                    // Content layout
                    let inner_rect = rect.shrink(10.0);
                    let text_rect = egui::Rect::from_min_max(
                        inner_rect.min,
                        egui::Pos2::new(inner_rect.max.x - 36.0, inner_rect.max.y),
                    );
                    let button_rect = egui::Rect::from_min_max(
                        egui::Pos2::new(inner_rect.max.x - 30.0, inner_rect.min.y + 8.0),
                        egui::Pos2::new(inner_rect.max.x, inner_rect.max.y - 8.0),
                    );

                    // Title (truncated, char-safe to avoid panics on multibyte titles)
                    let max_title_chars = ((text_rect.width() - 10.0) / 7.0) as usize;
                    let title: Cow<'_, str> = if window.title.chars().count() > max_title_chars {
                        let kept: String = window
                            .title
                            .chars()
                            .take(max_title_chars.saturating_sub(3))
                            .collect();
                        Cow::Owned(format!("{}...", kept))
                    } else {
                        Cow::Borrowed(&window.title)
                    };

                    // Draw title
                    ui.painter().text(
                        egui::Pos2::new(text_rect.min.x, text_rect.min.y + 2.0),
                        egui::Align2::LEFT_TOP,
                        title.as_ref(),
                        egui::FontId::proportional(14.0),
                        egui::Color32::WHITE,
                    );

                    // Draw exe name
                    ui.painter().text(
                        egui::Pos2::new(text_rect.min.x, text_rect.min.y + 20.0),
                        egui::Align2::LEFT_TOP,
                        &window.exe_name,
                        egui::FontId::proportional(11.0),
                        text_secondary,
                    );

                    // Add button (+ icon)
                    let btn_center = button_rect.center();
                    let btn_radius = 14.0;
                    let btn_rect =
                        egui::Rect::from_center_size(btn_center, Vec2::splat(btn_radius * 2.0));

                    let btn_response =
                        ui.interact(btn_rect, response.id.with("add_btn"), egui::Sense::click());
                    let btn_hovered = btn_response.hovered();

                    // Draw + button circle
                    ui.painter().circle_filled(
                        btn_center,
                        btn_radius,
                        if btn_hovered {
                            accent_color
                        } else {
                            egui::Color32::from_rgb(60, 60, 68)
                        },
                    );

                    // Draw + icon
                    let plus_color = if btn_hovered {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_rgb(180, 180, 190)
                    };
                    ui.painter().text(
                        btn_center,
                        egui::Align2::CENTER_CENTER,
                        egui_phosphor::regular::PLUS,
                        egui::FontId::proportional(14.0),
                        plus_color,
                    );

                    // Handle add button click
                    if btn_response.clicked() {
                        Self::add_window_to_canvas(
                            window,
                            preview_manager,
                            capture_coordinator,
                            canvas,
                        );
                    }

                    ui.add_space(4.0);
                }
            });

        if self.filtered_indices.is_empty()
            && self.filtered_spout_indices.is_empty()
            && !self.normalized_filter.is_empty()
        {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("No matching windows or Spout senders")
                        .size(13.0)
                        .color(text_secondary),
                );
            });
        }
    }

    fn update_filter(&mut self) {
        if !self.filter_dirty {
            return;
        }
        self.normalized_filter = self.search_filter.to_lowercase();
        self.filtered_indices.clear();
        self.filtered_indices.extend(
            self.windows
                .iter()
                .enumerate()
                .filter(|(_, window)| window.matches_filter(&self.normalized_filter))
                .map(|(index, _)| index),
        );
        self.filtered_spout_indices.clear();
        self.filtered_spout_indices.extend(
            self.spout
                .senders
                .iter()
                .enumerate()
                .filter(|(_, sender)| sender.matches_filter(&self.normalized_filter))
                .map(|(index, _)| index),
        );
        self.filter_dirty = false;
    }

    fn spout_section(
        &self,
        ui: &mut egui::Ui,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
        canvas: &CanvasState,
        palette: PickerPalette,
    ) {
        let text_secondary = palette.text_secondary;
        let accent = egui::Color32::from_rgb(107, 170, 75);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(egui_phosphor::regular::BROADCAST)
                    .size(13.0)
                    .color(if self.spout.is_present() {
                        accent
                    } else {
                        text_secondary
                    }),
            );
            ui.label(
                RichText::new("Spout2")
                    .size(12.0)
                    .color(egui::Color32::WHITE),
            );
            ui.label(
                RichText::new(self.spout.status_label())
                    .size(11.0)
                    .color(text_secondary),
            );
        });
        ui.add_space(4.0);

        if self.filtered_spout_indices.is_empty() {
            if self.normalized_filter.is_empty() {
                ui.label(
                    RichText::new(if self.spout.is_present() {
                        "Start a sender such as VTube Studio, then add it here."
                    } else {
                        "No Spout sender map or SpoutSettings key found."
                    })
                    .size(11.0)
                    .color(text_secondary),
                );
            }
            ui.add_space(8.0);
            return;
        }

        let row_height = 48.0;
        let list_height = (self.filtered_spout_indices.len() as f32 * (row_height + 4.0))
            .min(160.0)
            .max(row_height);
        let mut add_sender = None;
        egui::ScrollArea::vertical()
            .id_salt("spout_sender_list")
            .max_height(list_height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let available_width = ui.available_width();
                for &idx in &self.filtered_spout_indices {
                    let sender = &self.spout.senders[idx];
                    if draw_spout_sender_row(
                        ui,
                        sender,
                        available_width,
                        row_height,
                        palette,
                        self.spout.active_sender.as_deref(),
                    ) {
                        add_sender = Some(sender.clone());
                    }
                    ui.add_space(4.0);
                }
            });
        if let Some(sender) = add_sender {
            Self::add_spout_to_canvas(&sender, preview_manager, capture_coordinator, canvas);
        }
        ui.add_space(8.0);
    }

    /// Add a window to the canvas
    fn add_window_to_canvas(
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

        spawn_preview(
            window,
            preview_manager,
            capture_coordinator,
            position,
            Vec2::new(320.0, 240.0),
        );
    }

    fn add_spout_to_canvas(
        sender: &SpoutSender,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
        canvas: &CanvasState,
    ) {
        let preview_count = preview_manager.count();
        let offset = Vec2::new(
            (preview_count % 3) as f32 * 50.0,
            (preview_count / 3) as f32 * 50.0,
        );
        let position = Pos2::new(
            -canvas.pan.x + 50.0 + offset.x,
            -canvas.pan.y + 50.0 + offset.y,
        );
        let size = if sender.width > 0 && sender.height > 0 {
            let aspect = sender.width as f32 / sender.height as f32;
            Vec2::new(320.0, (320.0 / aspect).clamp(120.0, 360.0))
        } else {
            Vec2::new(320.0, 240.0)
        };
        let id =
            preview_manager.add_for_spout(sender.name.clone(), position, size, FpsPreset::Medium);
        capture_coordinator.start_spout_capture(id, sender.name.clone(), 30);
    }
}

fn draw_spout_sender_row(
    ui: &mut egui::Ui,
    sender: &SpoutSender,
    available_width: f32,
    row_height: f32,
    palette: PickerPalette,
    active_sender: Option<&str>,
) -> bool {
    let PickerPalette {
        text_secondary,
        card_bg,
        card_hover,
    } = palette;
    let accent_color = egui::Color32::from_rgb(107, 170, 75);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(available_width, row_height), egui::Sense::hover());
    let is_hovered = response.hovered();
    ui.painter().rect_filled(
        rect,
        Rounding::same(6.0),
        if is_hovered { card_hover } else { card_bg },
    );
    if is_hovered {
        ui.painter().rect_stroke(
            rect,
            Rounding::same(6.0),
            Stroke::new(1.0, egui::Color32::from_rgb(50, 50, 58)),
        );
    }

    let inner = rect.shrink2(Vec2::new(10.0, 6.0));
    let text_rect =
        egui::Rect::from_min_max(inner.min, egui::Pos2::new(inner.max.x - 36.0, inner.max.y));
    let is_active = active_sender == Some(sender.name.as_str());
    let title: Cow<'_, str> = if is_active {
        Cow::Owned(format!("{} (active)", sender.name))
    } else {
        Cow::Borrowed(&sender.name)
    };
    ui.painter().text(
        text_rect.min,
        egui::Align2::LEFT_TOP,
        title.as_ref(),
        egui::FontId::proportional(13.0),
        egui::Color32::WHITE,
    );
    ui.painter().text(
        egui::Pos2::new(text_rect.min.x, text_rect.min.y + 18.0),
        egui::Align2::LEFT_TOP,
        format!("{} · {}", sender.size_label(), sender.host_filename()),
        egui::FontId::proportional(11.0),
        text_secondary,
    );

    let btn_center = egui::Pos2::new(inner.max.x - 14.0, inner.center().y);
    let btn_radius = 14.0;
    let btn_rect = egui::Rect::from_center_size(btn_center, Vec2::splat(btn_radius * 2.0));
    let btn_response = ui.interact(
        btn_rect,
        response.id.with("add_spout"),
        egui::Sense::click(),
    );
    let btn_hovered = btn_response.hovered();
    ui.painter().circle_filled(
        btn_center,
        btn_radius,
        if btn_hovered {
            accent_color
        } else {
            egui::Color32::from_rgb(60, 60, 68)
        },
    );
    ui.painter().text(
        btn_center,
        egui::Align2::CENTER_CENTER,
        egui_phosphor::regular::PLUS,
        egui::FontId::proportional(14.0),
        if btn_hovered {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_rgb(180, 180, 190)
        },
    );
    let added = btn_response.clicked();
    btn_response.on_hover_text(format!(
        "Capture {}\n{}",
        sender.name,
        if sender.host_path.is_empty() {
            "Spout2 sender"
        } else {
            sender.host_path.as_str()
        }
    ));
    added
}

/// Create a preview for `window` at `position`/`size` and start capturing it.
/// Shared by the sidebar picker's "+" button and the canvas right-click
/// quick-add popup so both add windows the same way.
pub fn spawn_preview(
    window: &WindowInfo,
    preview_manager: &mut PreviewManager,
    capture_coordinator: &mut CaptureCoordinator,
    position: Pos2,
    size: Vec2,
) {
    let id = preview_manager.add_for_window(
        window.hwnd,
        window.process_id,
        window.title.clone(),
        position,
        size,
    );

    capture_coordinator.start_capture(id, window.hwnd, window.title.clone(), 30);
}

impl Default for WindowPicker {
    fn default() -> Self {
        Self::new()
    }
}
