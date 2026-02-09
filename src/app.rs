use eframe::egui::{self, Vec2, Pos2};
use crate::canvas::CanvasState;
use crate::preview::{PreviewManager, PreviewLayout};
use crate::window_picker::{WindowPicker, enumerate_windows};
use crate::capture::CaptureCoordinator;
use crate::persistence::{Storage, SavedLayout, CanvasLayout};
use crate::tray::TrayManager;
use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
use windows::core::w;

/// Main application state
pub struct PluriviewApp {
    /// Canvas state (pan, zoom, selection)
    pub canvas: CanvasState,

    /// Manages all preview windows
    pub preview_manager: PreviewManager,

    /// Window picker panel state
    pub window_picker: WindowPicker,

    /// Capture coordinator for managing window captures
    pub capture_coordinator: CaptureCoordinator,

    /// Is the window picker panel open?
    pub picker_open: bool,

    /// Storage for persistence
    storage: Option<Storage>,

    /// System tray manager
    tray_manager: Option<TrayManager>,

    /// Has the window HWND been set for the tray manager?
    hwnd_set: bool,

    /// Show About dialog
    show_about: bool,

    /// Show Keyboard Shortcuts dialog
    show_shortcuts: bool,
}

impl PluriviewApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let storage = Storage::new();
        let tray_manager = TrayManager::new();

        #[cfg(debug_assertions)]
        if tray_manager.is_some() {
            println!("System tray initialized");
        } else {
            eprintln!("Failed to initialize system tray");
        }

        let mut app = Self {
            canvas: CanvasState::default(),
            preview_manager: PreviewManager::new(),
            window_picker: WindowPicker::new(),
            capture_coordinator: CaptureCoordinator::new(),
            picker_open: true,
            storage,
            tray_manager,
            hwnd_set: false,
            show_about: false,
            show_shortcuts: false,
        };

        // Try to load autosave
        app.load_autosave();

        app
    }

    /// Set the window HWND for the tray manager (call once after window is created)
    fn setup_tray_hwnd(&mut self) {
        if self.hwnd_set || self.tray_manager.is_none() {
            return;
        }

        // Find our window by title
        if let Ok(hwnd) = unsafe { FindWindowW(None, w!("Pluriview")) } {
            if hwnd.0 as isize != 0 {
                TrayManager::set_window_hwnd(hwnd.0 as isize);
                self.hwnd_set = true;
                #[cfg(debug_assertions)]
                println!("Set tray HWND: {:?}", hwnd.0);
            }
        }
    }

    /// Load the autosave layout if it exists
    fn load_autosave(&mut self) {
        if let Some(storage) = &self.storage {
            if let Ok(layout) = storage.load_autosave() {
                self.apply_layout(&layout);
                #[cfg(debug_assertions)]
                println!("Loaded autosave with {} previews", layout.previews.len());
            }
        }
    }

    /// Save the current layout to autosave
    fn save_autosave(&self) {
        if let Some(storage) = &self.storage {
            let layout = self.create_layout("autosave".to_string());
            if let Err(e) = storage.save_autosave(&layout) {
                eprintln!("Failed to save autosave: {}", e);
            } else {
                #[cfg(debug_assertions)]
                println!("Saved autosave with {} previews", layout.previews.len());
            }
        }
    }

    /// Create a SavedLayout from current state
    fn create_layout(&self, name: String) -> SavedLayout {
        let mut layout = SavedLayout::new(name);

        // Save canvas state
        layout.canvas = CanvasLayout {
            pan: (self.canvas.pan.x, self.canvas.pan.y),
            zoom: self.canvas.zoom,
            show_grid: self.canvas.show_grid,
        };

        // Save all previews
        layout.previews = self.preview_manager.all()
            .map(|p| PreviewLayout::from(p))
            .collect();

        layout
    }

    /// Apply a SavedLayout to restore state
    fn apply_layout(&mut self, layout: &SavedLayout) {
        // Clear existing state
        self.preview_manager.clear();
        self.capture_coordinator.stop_all();

        // Restore canvas state
        self.canvas.pan = Vec2::new(layout.canvas.pan.0, layout.canvas.pan.1);
        self.canvas.zoom = layout.canvas.zoom;
        self.canvas.show_grid = layout.canvas.show_grid;

        // Enumerate current windows to find matching ones
        let current_windows = enumerate_windows();

        // Restore previews
        for preview_layout in &layout.previews {
            // Try to find a matching window by title
            let matching_window = current_windows.iter()
                .find(|w| w.title == preview_layout.window_title);

            if let Some(window_info) = matching_window {
                // Create preview with saved position/size
                let id = self.preview_manager.add_with_window(
                    window_info.title.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    window_info.hwnd,
                    preview_layout.fps_preset,
                    preview_layout.z_order,
                );

                // Start capture
                self.capture_coordinator.start_capture(
                    id,
                    window_info.hwnd,
                    window_info.title.clone(),
                    preview_layout.fps_preset.as_u32(),
                );

                // Restore crop region if it was saved
                if let Some(crop) = preview_layout.crop_uv {
                    if let Some(preview) = self.preview_manager.get_mut(id) {
                        preview.crop_uv = Some(crop);
                    }
                }

                #[cfg(debug_assertions)]
                println!("Restored preview: {}", window_info.title);
            } else {
                #[cfg(debug_assertions)]
                println!("Window not found: {}", preview_layout.window_title);
            }
        }
    }
}

impl eframe::App for PluriviewApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Auto-save on exit
        self.save_autosave();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Set up tray HWND on first frame (window now exists)
        self.setup_tray_hwnd();

        // Process any pending captured frames
        self.capture_coordinator.process_frames(&mut self.preview_manager, ctx);

        // Minimal Void: Very dark, minimal menu bar
        egui::TopBottomPanel::top("top_panel")
            .frame(egui::Frame::none()
                .fill(egui::Color32::from_rgb(13, 13, 13))
                .inner_margin(egui::Margin::symmetric(8.0, 4.0)))
            .show(ctx, |ui| {
                ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                ui.visuals_mut().widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(30, 30, 35);
                ui.visuals_mut().widgets.active.weak_bg_fill = egui::Color32::from_rgb(40, 40, 45);

                egui::menu::bar(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("Save Layout Now").clicked() {
                            self.save_autosave();
                            ui.close_menu();
                        }
                        if ui.button("Reload Layout").clicked() {
                            self.load_autosave();
                            ui.close_menu();
                        }
                        ui.separator();
                        if self.tray_manager.is_some() {
                            if ui.button("Minimize to Tray").clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                                ui.close_menu();
                            }
                            ui.separator();
                        }
                        if ui.button("Exit").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });

                    ui.menu_button("View", |ui| {
                        if ui.checkbox(&mut self.picker_open, "Window Picker").clicked() {
                            ui.close_menu();
                        }
                        if ui.checkbox(&mut self.canvas.show_grid, "Show Grid (G)").clicked() {
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Reset View").clicked() {
                            self.canvas.reset();
                            ui.close_menu();
                        }
                    });

                    ui.menu_button("Help", |ui| {
                        if ui.button("Keyboard Shortcuts").clicked() {
                            self.show_shortcuts = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("About").clicked() {
                            self.show_about = true;
                            ui.close_menu();
                        }
                    });
                });
            });

        // Minimal Void: Dark sidebar
        if self.picker_open {
            egui::SidePanel::left("window_picker_panel")
                .default_width(250.0)
                .min_width(200.0)
                .max_width(400.0)
                .frame(egui::Frame::none()
                    .fill(egui::Color32::from_rgb(18, 18, 18))
                    .inner_margin(egui::Margin::same(8.0)))
                .show(ctx, |ui| {
                    self.window_picker.ui(
                        ui,
                        &mut self.preview_manager,
                        &mut self.capture_coordinator,
                        &self.canvas
                    );
                });
        }

        // Minimal Void: No status bar - floating indicator is drawn in the canvas

        // Minimal Void: Main canvas area with dark background
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::from_rgb(13, 13, 13)))
            .show(ctx, |ui| {
                self.canvas.ui(ui, &mut self.preview_manager, &mut self.capture_coordinator, ctx);
            });

        // Handle global keyboard shortcuts
        ctx.input(|i| {
            // G - Toggle grid
            if i.key_pressed(egui::Key::G) && !i.modifiers.ctrl && !i.modifiers.alt {
                self.canvas.show_grid = !self.canvas.show_grid;
            }
            // F1 - Show keyboard shortcuts
            if i.key_pressed(egui::Key::F1) {
                self.show_shortcuts = true;
            }
        });

        // About dialog
        if self.show_about {
            egui::Window::new("About Pluriview")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .frame(egui::Frame::window(&ctx.style())
                    .fill(egui::Color32::from_rgb(25, 25, 28)))
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(10.0);
                        ui.heading("Pluriview");
                        ui.label("Version 0.1.0");
                        ui.add_space(10.0);
                        ui.label("Live window preview application");
                        ui.label("with infinite canvas");
                        ui.add_space(15.0);
                        ui.label(egui::RichText::new("Created by Javier").weak());
                        ui.add_space(15.0);
                        if ui.button("Close").clicked() {
                            self.show_about = false;
                        }
                        ui.add_space(5.0);
                    });
                });
        }

        // Keyboard Shortcuts dialog
        if self.show_shortcuts {
            egui::Window::new("Keyboard Shortcuts")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .frame(egui::Frame::window(&ctx.style())
                    .fill(egui::Color32::from_rgb(25, 25, 28)))
                .show(ctx, |ui| {
                    ui.add_space(5.0);

                    egui::Grid::new("shortcuts_grid")
                        .num_columns(2)
                        .spacing([40.0, 8.0])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("Navigation").strong());
                            ui.label("");
                            ui.end_row();

                            ui.label("Pan canvas");
                            ui.label(egui::RichText::new("Middle Mouse / Alt+Drag").weak());
                            ui.end_row();

                            ui.label("Zoom");
                            ui.label(egui::RichText::new("Scroll Wheel").weak());
                            ui.end_row();

                            ui.label("Toggle grid");
                            ui.label(egui::RichText::new("G").weak());
                            ui.end_row();

                            ui.add_space(10.0);
                            ui.end_row();

                            ui.label(egui::RichText::new("Selection").strong());
                            ui.label("");
                            ui.end_row();

                            ui.label("Select all");
                            ui.label(egui::RichText::new("Ctrl+A").weak());
                            ui.end_row();

                            ui.label("Multi-select");
                            ui.label(egui::RichText::new("Ctrl+Click").weak());
                            ui.end_row();

                            ui.label("Delete selected");
                            ui.label(egui::RichText::new("Delete").weak());
                            ui.end_row();

                            ui.add_space(10.0);
                            ui.end_row();

                            ui.label(egui::RichText::new("Editing").strong());
                            ui.label("");
                            ui.end_row();

                            ui.label("Resize preview");
                            ui.label(egui::RichText::new("Drag corners/edges").weak());
                            ui.end_row();

                            ui.label("Crop preview");
                            ui.label(egui::RichText::new("Alt+Drag corners").weak());
                            ui.end_row();

                            ui.label("Context menu");
                            ui.label(egui::RichText::new("Right-click").weak());
                            ui.end_row();

                            ui.add_space(10.0);
                            ui.end_row();

                            ui.label(egui::RichText::new("Other").strong());
                            ui.label("");
                            ui.end_row();

                            ui.label("Show this help");
                            ui.label(egui::RichText::new("F1").weak());
                            ui.end_row();
                        });

                    ui.add_space(15.0);
                    ui.vertical_centered(|ui| {
                        if ui.button("Close").clicked() {
                            self.show_shortcuts = false;
                        }
                    });
                    ui.add_space(5.0);
                });
        }

        // Request continuous repaint for live previews and tray event processing
        // We always request repaint to ensure tray events are processed even when hidden
        ctx.request_repaint();
    }
}
