use super::{enumerate_windows, WindowInfo};
use crate::canvas::CanvasState;
use crate::capture::CaptureCoordinator;
use crate::preview::{FpsPreset, PreviewManager};
use crate::spout::{self, SpoutDetection, SpoutSender};
use crate::ui_theme;
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

        let card_bg = ui_theme::SURFACE;
        let card_hover = ui_theme::HOVER;
        let accent_color = ui_theme::GOLD;
        let text_secondary = ui_theme::SECONDARY;
        let search_bg = ui_theme::PANEL;

        ui.add_space(4.0);
        ui.label(
            RichText::new("Window picker")
                .size(19.0)
                .strong()
                .color(ui_theme::TEXT),
        );
        ui.label(
            RichText::new("Add a live source to your canvas")
                .size(12.0)
                .color(text_secondary),
        );
        ui.add_space(12.0);

        // Modern search box with rounded corners
        let search_frame = egui::Frame::none()
            .fill(search_bg)
            .stroke(Stroke::new(1.0, ui_theme::BORDER))
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
                let show_clear = !self.search_filter.is_empty();
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.search_filter)
                        .desired_width(
                            (ui.available_width() - if show_clear { 28.0 } else { 0.0 }).max(20.0),
                        )
                        .hint_text(RichText::new("Search sources…").color(text_secondary))
                        .frame(false),
                );
                if response.changed() {
                    self.filter_dirty = true;
                }
                if show_clear
                    && ui
                        .add_sized(
                            [20.0, 20.0],
                            egui::Button::new(egui_phosphor::regular::X).frame(false),
                        )
                        .on_hover_text("Clear search")
                        .clicked()
                {
                    self.search_filter.clear();
                    self.filter_dirty = true;
                    response.request_focus();
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
        // Active senders are useful here; absent Spout installations should not
        // push the everyday window list down with implementation details.
        if self.spout.is_present() || !self.filtered_spout_indices.is_empty() {
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
        }

        // Window count and refresh indicator
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("WINDOWS  ·  {}", self.filtered_indices.len()))
                    .size(12.0)
                    .strong()
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

        // Render empty states before the scroll area consumes the panel height.
        if self.filtered_indices.is_empty() {
            ui.add_space(16.0);
            ui.label(
                RichText::new(if self.normalized_filter.is_empty() {
                    "No windows available"
                } else {
                    "No matching windows"
                })
                .color(ui_theme::TEXT),
            );
            ui.label(
                RichText::new(if self.normalized_filter.is_empty() {
                    "Open an application, then refresh the list."
                } else {
                    "Try another title or application name."
                })
                .size(12.0)
                .color(text_secondary),
            );
        }

        // Window list with fixed-height row virtualization, so only visible
        // cards allocate egui widgets and text each frame.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, 64.0, self.filtered_indices.len(), |ui, row_range| {
                let available_width = ui.available_width();

                for row in row_range {
                    let idx = self.filtered_indices[row];
                    let window = &self.windows[idx];

                    // Card frame
                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(available_width, 64.0),
                        egui::Sense::hover(),
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
                            Stroke::new(1.0, ui_theme::BORDER),
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

                    ui_theme::clipped_text(
                        ui,
                        egui::Rect::from_min_size(
                            text_rect.min,
                            Vec2::new(text_rect.width(), 20.0),
                        ),
                        if window.title.is_empty() {
                            &window.exe_name
                        } else {
                            &window.title
                        },
                        14.0,
                        ui_theme::TEXT,
                    );
                    ui_theme::clipped_text(
                        ui,
                        egui::Rect::from_min_size(
                            text_rect.min + Vec2::new(0.0, 23.0),
                            Vec2::new(text_rect.width(), 18.0),
                        ),
                        &window.exe_name,
                        12.0,
                        text_secondary,
                    );
                    response
                        .clone()
                        .on_hover_text(format!("{}\n{}", window.title, window.exe_name));

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
                            ui_theme::GOLD_DARK
                        },
                    );

                    // Draw + icon
                    let plus_color = if btn_hovered {
                        ui_theme::PANEL
                    } else {
                        ui_theme::GOLD
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
                    btn_response.on_hover_text("Add window to canvas");
                }
            });
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
        let accent = ui_theme::GOLD;
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
                RichText::new("SPOUT2")
                    .size(12.0)
                    .strong()
                    .color(ui_theme::TEXT),
            );
            ui.label(
                RichText::new(self.spout.status_label())
                    .size(12.0)
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
                        "Start a Spout application to see its sources here."
                    })
                    .size(12.0)
                    .color(text_secondary),
                );
            }
            ui.add_space(8.0);
            return;
        }

        let row_height = 56.0;
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
    let accent_color = ui_theme::GOLD;
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
            Stroke::new(1.0, ui_theme::BORDER),
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
    ui_theme::clipped_text(
        ui,
        egui::Rect::from_min_size(text_rect.min, Vec2::new(text_rect.width(), 20.0)),
        title.as_ref(),
        14.0,
        ui_theme::TEXT,
    );
    ui_theme::clipped_text(
        ui,
        egui::Rect::from_min_size(
            text_rect.min + Vec2::new(0.0, 23.0),
            Vec2::new(text_rect.width(), 18.0),
        ),
        &format!("{} · {}", sender.size_label(), sender.host_filename()),
        12.0,
        text_secondary,
    );
    response.clone().on_hover_text(title.as_ref());

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
            ui_theme::GOLD_DARK
        },
    );
    ui.painter().text(
        btn_center,
        egui::Align2::CENTER_CENTER,
        egui_phosphor::regular::PLUS,
        egui::FontId::proportional(14.0),
        if btn_hovered {
            ui_theme::PANEL
        } else {
            ui_theme::GOLD
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
    if let Some(preview) = preview_manager.get_mut(id) {
        preview.window_exe = Some(window.exe_name.clone());
    }

    capture_coordinator.start_capture(id, window.hwnd, window.title.clone(), 30);
}

impl Default for WindowPicker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(
        picker: &mut WindowPicker,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        // Keep fixtures independent of applications open on the test machine.
        picker.last_refresh = std::time::Instant::now();
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(240.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    picker.ui(
                        ui,
                        &mut PreviewManager::new(),
                        &mut CaptureCoordinator::new(),
                        &CanvasState::default(),
                    );
                });
            },
        )
    }

    #[test]
    fn empty_search_feedback_is_visible_at_minimum_picker_width() {
        let ctx = egui::Context::default();
        ui_theme::install(&ctx);
        let mut picker = WindowPicker::new();
        picker.search_filter = "missing source".to_owned();
        let output = render(&mut picker, &ctx, vec![]);
        let shape = output
            .shapes
            .iter()
            .find(|shape| {
                matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text() == "No matching windows")
            })
            .unwrap();
        let egui::Shape::Text(text) = &shape.shape else {
            unreachable!()
        };
        assert!(shape
            .clip_rect
            .contains_rect(egui::Rect::from_min_size(text.pos, text.galley.size())));
    }

    #[test]
    fn clear_search_button_restores_the_filtered_window_list() {
        let ctx = egui::Context::default();
        ui_theme::install(&ctx);
        let mut picker = WindowPicker::new();
        picker.windows.push(WindowInfo::new(
            0,
            "Sample window".to_owned(),
            0,
            "sample.exe".to_owned(),
        ));
        picker.search_filter = "missing source".to_owned();
        let output = render(&mut picker, &ctx, vec![]);
        assert!(picker.filtered_indices.is_empty());
        let pos = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == egui_phosphor::regular::X => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap();
        render(&mut picker, &ctx, vec![egui::Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            render(
                &mut picker,
                &ctx,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        assert!(picker.search_filter.is_empty());
        assert_eq!(picker.filtered_indices, vec![0]);
    }
}
