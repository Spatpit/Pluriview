use eframe::egui::{self, Vec2, Pos2};
use std::time::{Duration, Instant};
#[cfg(debug_assertions)]
use crate::privacy;
use crate::canvas::{BrowserAction, CanvasState};
use crate::preview::{PreviewManager, PreviewLayout, PreviewId, FpsPreset, WindowHandle};
use crate::window_picker::{WindowPicker, WindowInfo, enumerate_windows, spawn_preview};
use crate::capture::CaptureCoordinator;
use crate::persistence::{Storage, SavedLayout, CanvasLayout};
use crate::tray::TrayManager;
use crate::overlay::RegionSelector;
#[cfg(windows)]
use crate::browser::{self, normalize_url, BrowserManager};
#[cfg(windows)]
use windows::core::HSTRING;
#[cfg(windows)]
use windows::Win32::Foundation::HWND;
#[cfg(windows)]
use windows::Win32::UI::Shell::ShellExecuteW;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, SW_SHOWNORMAL};
use wry::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

/// How long after activating a browser we skip the focus-loss check, so the
/// WebView has time to actually take the foreground.
const BROWSER_FOCUS_GRACE: Duration = Duration::from_millis(300);

/// How many recent browser URLs to keep for the Add Browser dialog.
const MAX_RECENT_URLS: usize = 8;

/// Canvas right-click "Add Window..." popup: a small searchable list shown
/// at the click position so windows can be added without the sidebar.
struct QuickAddPopup {
    /// Where to place the new preview (canvas coordinates).
    canvas_pos: Pos2,
    /// Where to anchor the popup (screen coordinates).
    screen_pos: Pos2,
    /// Snapshot of open windows, taken when the popup was opened.
    windows: Vec<WindowInfo>,
    search: String,
}

struct AddBrowserDialog {
    position: Pos2,
    url: String,
    error: Option<String>,
    /// Existing browser tile being retargeted ("Change URL..."); None = create new.
    target: Option<PreviewId>,
    /// The URL field grabs focus once when the dialog opens.
    focused: bool,
}

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

    /// Active region selector overlay (if any)
    region_selector: Option<RegionSelector>,

    /// Preview ID that the region selector is for
    region_select_preview_id: Option<PreviewId>,

    /// Active canvas right-click "Add Window..." popup, if any.
    quick_add: Option<QuickAddPopup>,

    /// Main window HWND, cached from eframe on the first frame.
    main_hwnd: Option<isize>,

    /// Recently added browser URLs, newest first.
    recent_urls: Vec<String>,

    #[cfg(windows)]
    browser: BrowserManager,
    #[cfg(windows)]
    add_browser: Option<AddBrowserDialog>,
    /// When the current browser interaction mode started (focus grace period).
    #[cfg(windows)]
    browser_activated_at: Option<Instant>,
}

impl PluriviewApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        // Register phosphor icon glyphs alongside the default font so we can
        // use crisp vector icons instead of emoji/text glyphs in the UI.
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        _cc.egui_ctx.set_fonts(fonts);

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
            region_selector: None,
            region_select_preview_id: None,
            quick_add: None,
            main_hwnd: None,
            recent_urls: Vec::new(),
            #[cfg(windows)]
            browser: BrowserManager::new(),
            #[cfg(windows)]
            add_browser: None,
            #[cfg(windows)]
            browser_activated_at: None,
        };

        // Try to load autosave
        app.load_autosave();

        app
    }

    /// Create a browser tile: WebView host, preview, and capture session.
    /// Used by the Add Browser dialog, layout restore, and undo.
    #[cfg(windows)]
    fn create_browser_tile(
        &mut self,
        url: &str,
        position: Pos2,
        size: Vec2,
        fps: FpsPreset,
    ) -> Result<PreviewId, String> {
        let url = normalize_url(url).map_err(str::to_owned)?;

        // Reserve the preview first so the host and capture share its ID.
        let id = self
            .preview_manager
            .add_for_window(0, std::process::id(), url.clone(), position, size);

        match self.browser.create(id, &url) {
            Ok(hwnd) => {
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    preview.window_handle = Some(WindowHandle {
                        hwnd,
                        process_id: std::process::id(),
                    });
                    preview.capture_active = true;
                    preview.browser_url = Some(url.clone());
                    preview.set_fps_preset(fps);
                }
                self.capture_coordinator
                    .start_capture(id, hwnd, url.clone(), fps.as_u32());
                self.remember_recent_url(&url);
                Ok(id)
            }
            Err(error) => {
                self.preview_manager.remove(id);
                Err(error)
            }
        }
    }

    fn remember_recent_url(&mut self, url: &str) {
        self.recent_urls.retain(|u| u != url);
        self.recent_urls.insert(0, url.to_owned());
        self.recent_urls.truncate(MAX_RECENT_URLS);
    }

    #[cfg(windows)]
    fn add_browser_ui(&mut self, ctx: &egui::Context) {
        let mut submit = None;
        let mut cancel = false;

        if let Some(dialog) = self.add_browser.as_mut() {
            let editing = dialog.target.is_some();
            let recent_urls = &self.recent_urls;
            egui::Window::new(if editing { "Change URL" } else { "Add Browser" })
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Paste a website URL");
                    let response = ui.add_sized(
                        [420.0, 24.0],
                        egui::TextEdit::singleline(&mut dialog.url)
                            .hint_text("twitch.tv/channel or https://kick.com/channel"),
                    );
                    if !dialog.focused {
                        response.request_focus();
                        dialog.focused = true;
                    }
                    if let Some(error) = &dialog.error {
                        ui.colored_label(egui::Color32::from_rgb(255, 100, 100), error);
                    }

                    // Pressing Enter in a TextEdit surrenders focus that same
                    // frame, so lost_focus + Enter is the reliable submit check.
                    let submitted = response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));

                    ui.horizontal(|ui| {
                        let label = if editing { "Load" } else { "Add" };
                        if ui.button(label).clicked() || submitted {
                            submit = Some((dialog.url.clone(), dialog.position, dialog.target));
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });

                    if !recent_urls.is_empty() {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("Recent").weak().small());
                        for url in recent_urls.iter().take(5) {
                            if ui
                                .add(egui::Button::new(egui::RichText::new(url).size(11.5)).frame(false))
                                .clicked()
                            {
                                submit = Some((url.clone(), dialog.position, dialog.target));
                            }
                        }
                    }
                });
        }

        if cancel {
            self.add_browser = None;
        } else if let Some((url, position, target)) = submit {
            let result = match target {
                // Retarget an existing tile: navigate its WebView in place.
                Some(id) => normalize_url(&url).map_err(str::to_owned).map(|url| {
                    if let Some(host) = self.browser.get(id) {
                        host.load(&url);
                    }
                    if let Some(preview) = self.preview_manager.get_mut(id) {
                        preview.browser_url = Some(url.clone());
                        preview.title = url.clone();
                    }
                    self.remember_recent_url(&url);
                }),
                None => self
                    .create_browser_tile(&url, position, Vec2::new(640.0, 360.0), FpsPreset::Medium)
                    .map(|_| ()),
            };
            match result {
                Ok(()) => self.add_browser = None,
                Err(error) => {
                    if let Some(dialog) = self.add_browser.as_mut() {
                        dialog.error = Some(error);
                        // Put the caret back so the user can correct the URL.
                        dialog.focused = false;
                    }
                }
            }
        }
    }

    /// Apply an action queued by a browser tile's hover controls / context menu.
    #[cfg(windows)]
    fn handle_browser_action(&mut self, ctx: &egui::Context, id: PreviewId, action: BrowserAction) {
        match action {
            BrowserAction::Back => {
                if let Some(host) = self.browser.get(id) {
                    host.go_back();
                }
            }
            BrowserAction::Forward => {
                if let Some(host) = self.browser.get(id) {
                    host.go_forward();
                }
            }
            BrowserAction::Reload => {
                if let Some(host) = self.browser.get(id) {
                    host.reload();
                }
            }
            BrowserAction::ToggleMute => {
                if let Some(host) = self.browser.get_mut(id) {
                    let muted = !host.is_muted();
                    if host.set_muted(muted).is_ok() {
                        if let Some(preview) = self.preview_manager.get_mut(id) {
                            preview.browser_muted = muted;
                        }
                    }
                }
            }
            BrowserAction::CopyUrl => {
                if let Some(host) = self.browser.get(id) {
                    ctx.copy_text(host.current_url());
                }
            }
            BrowserAction::OpenExternal => {
                if let Some(host) = self.browser.get(id) {
                    let url = HSTRING::from(host.current_url());
                    unsafe {
                        ShellExecuteW(
                            None,
                            windows::core::w!("open"),
                            &url,
                            None,
                            None,
                            SW_SHOWNORMAL,
                        );
                    }
                }
            }
            BrowserAction::EditUrl => {
                let current = self
                    .browser
                    .get(id)
                    .map(|host| host.current_url())
                    .unwrap_or_default();
                self.add_browser = Some(AddBrowserDialog {
                    position: Pos2::ZERO,
                    url: current,
                    error: None,
                    target: Some(id),
                    focused: false,
                });
            }
        }
    }

    /// Where a browser host window should sit for tile `id`, in egui points
    /// (window client coordinates). Inset a little so the canvas' accent
    /// outline stays visible around the live window. None when the tile is
    /// fully outside the canvas area.
    #[cfg(windows)]
    fn browser_tile_rect(&self, id: PreviewId, canvas_rect: egui::Rect) -> Option<egui::Rect> {
        let preview = self.preview_manager.get(id)?;
        let rect = self.canvas.canvas_rect_to_screen(preview.rect(), canvas_rect);
        if !rect.intersects(canvas_rect) {
            return None;
        }
        let inset = 3.0_f32.min(rect.width() / 4.0).min(rect.height() / 4.0);
        Some(rect.shrink(inset.max(0.0)))
    }

    /// Per-frame browser housekeeping. Runs after the canvas UI so tile
    /// rects and double-click state are fresh.
    #[cfg(windows)]
    fn browser_frame(&mut self, ctx: &egui::Context) {
        // Mirror page titles and current URLs onto the tiles so the hover
        // overlay shows "lofi hip hop radio..." instead of the raw URL and
        // layouts save where the user actually navigated.
        let mut updates = Vec::new();
        for (id, host) in self.browser.iter_mut() {
            let update = host.poll();
            if update.title.is_some() || update.url.is_some() {
                updates.push((*id, update));
            }
        }
        for (id, update) in updates {
            if let Some(preview) = self.preview_manager.get_mut(id) {
                if let Some(title) = update.title {
                    if !title.is_empty() {
                        preview.title = title;
                    }
                }
                if let Some(url) = update.url {
                    preview.browser_url = Some(url);
                }
            }
        }

        // Interaction-mode upkeep for the (single) active host.
        if let Some(active_id) = self.browser.active_id() {
            let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
            let escape = browser::escape_pressed();
            let in_grace = self
                .browser_activated_at
                .is_some_and(|at| at.elapsed() < BROWSER_FOCUS_GRACE);
            let owns_focus = self
                .browser
                .get(active_id)
                .is_some_and(|host| host.owns_foreground());
            let tile_rect = self
                .canvas
                .last_screen_rect
                .and_then(|canvas_rect| self.browser_tile_rect(active_id, canvas_rect));

            let should_park =
                escape || minimized || tile_rect.is_none() || (!owns_focus && !in_grace);

            if should_park {
                if let Some(host) = self.browser.get_mut(active_id) {
                    host.park();
                }
                self.browser_activated_at = None;
                // On Escape the (now offscreen) WebView still holds focus;
                // hand it back to the main window so keyboard input works.
                if escape && !minimized {
                    if let Some(hwnd) = self.main_hwnd {
                        unsafe {
                            let _ = SetForegroundWindow(HWND(hwnd as *mut _));
                        }
                    }
                }
            } else if let (Some(hwnd), Some(rect)) = (self.main_hwnd, tile_rect) {
                // Glue the live host to its tile so panning/zooming the
                // canvas or moving the window keeps them in lockstep.
                if let Some(host) = self.browser.get_mut(active_id) {
                    host.place(HWND(hwnd as *mut _), rect, ctx.pixels_per_point(), false);
                }
            }
        }

        // Let the canvas outline the interactive tile in the accent color.
        self.canvas.interactive_browser = self.browser.active_id();
    }

    /// Set the window HWND for the tray manager (call once after window is created)
    fn setup_tray_hwnd(&mut self) {
        if self.hwnd_set || self.tray_manager.is_none() {
            return;
        }

        if let Some(hwnd) = self.main_hwnd {
            TrayManager::set_window_hwnd(hwnd);
            self.hwnd_set = true;
            #[cfg(debug_assertions)]
            println!("Set tray HWND: {:?}", hwnd);
        }
    }

    /// Custom title bar (we run with `with_decorations(false)` so the OS
    /// doesn't draw its own white title bar over our dark theme).
    fn title_bar_ui(&mut self, ctx: &egui::Context) {
        let bg = egui::Color32::from_rgb(13, 13, 13);
        let is_maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));

        egui::TopBottomPanel::top("custom_title_bar")
            .frame(egui::Frame::none().fill(bg))
            .exact_height(34.0)
            .show(ctx, |ui| {
                let title_bar_rect = ui.max_rect();

                // Background drag handle, allocated FIRST so the buttons
                // (added after) take interaction priority where they overlap.
                let drag_response = ui.interact(
                    title_bar_rect,
                    egui::Id::new("title_bar_drag"),
                    egui::Sense::click_and_drag(),
                );
                if drag_response.double_clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
                } else if drag_response.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(title_bar_rect), |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.add_space(10.0);
                        let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                        ui.painter().circle_filled(
                            dot_rect.center(),
                            4.0,
                            egui::Color32::from_rgb(107, 170, 75),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Pluriview")
                                .size(13.0)
                                .color(egui::Color32::from_rgb(170, 170, 175)),
                        );
                        ui.add_space(16.0);
                        // File / View / Help, inline next to the app name.
                        self.menu_bar(ui, ctx);
                    });
                });

                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(title_bar_rect), |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;

                        let btn_size = Vec2::new(44.0, 34.0);

                        let close = ui.add_sized(
                            btn_size,
                            egui::Button::new(egui::RichText::new(egui_phosphor::regular::X).size(14.0))
                                .frame(false),
                        );
                        if close.hovered() {
                            ui.painter().rect_filled(close.rect, 0.0, egui::Color32::from_rgb(196, 43, 28));
                            ui.painter().text(
                                close.rect.center(),
                                egui::Align2::CENTER_CENTER,
                                egui_phosphor::regular::X,
                                egui::FontId::proportional(14.0),
                                egui::Color32::WHITE,
                            );
                        }
                        if close.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }

                        let max_icon = if is_maximized {
                            egui_phosphor::regular::COPY
                        } else {
                            egui_phosphor::regular::SQUARE
                        };
                        let maximize = ui.add_sized(
                            btn_size,
                            egui::Button::new(egui::RichText::new(max_icon).size(12.0))
                                .frame(false),
                        );
                        if maximize.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
                        }

                        let minimize = ui.add_sized(
                            btn_size,
                            egui::Button::new(egui::RichText::new(egui_phosphor::regular::MINUS).size(14.0))
                                .frame(false),
                        );
                        if minimize.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                    });
                });
            });
    }

    /// The File / View / Help menus. Rendered inline in the title bar next
    /// to the app name (Minimal Void: one unified dark strip, no separate
    /// menu-bar row).
    fn menu_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
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
    }

    /// We turned off OS decorations for the custom title bar, which also
    /// removes the native resize border. Re-implement it: a thin hit-band
    /// along each edge that shows a resize cursor and starts an OS-driven
    /// resize drag (so resizing still feels native).
    fn handle_frameless_resize(&self, ctx: &egui::Context) {
        use egui::viewport::ResizeDirection as RD;

        if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
            return;
        }
        // Don't fight with widgets that already want the pointer (e.g. a
        // preview's own resize handles) by only acting near the window edge.
        let border = 6.0;
        // The custom title bar owns the entire top strip (drag-to-move plus
        // the min/max/close buttons) — never treat that area as a resize
        // zone, or a click on a title bar button can also start a native
        // resize drag and leave the window stuck at a tiny size.
        let title_bar_height = 34.0;
        let rect = ctx.input(|i| i.screen_rect());
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return; };

        if pos.y < rect.min.y + title_bar_height {
            return;
        }

        let on_left = pos.x <= rect.min.x + border;
        let on_right = pos.x >= rect.max.x - border;
        let on_bottom = pos.y >= rect.max.y - border;

        let direction = match (on_left, on_right, on_bottom) {
            (true, _, true) => Some(RD::SouthWest),
            (_, true, true) => Some(RD::SouthEast),
            (true, false, false) => Some(RD::West),
            (false, true, false) => Some(RD::East),
            (false, false, true) => Some(RD::South),
            _ => None,
        };

        let Some(direction) = direction else { return; };

        let cursor = match direction {
            RD::NorthWest | RD::SouthEast => egui::CursorIcon::ResizeNwSe,
            RD::NorthEast | RD::SouthWest => egui::CursorIcon::ResizeNeSw,
            RD::North | RD::South => egui::CursorIcon::ResizeVertical,
            RD::East | RD::West => egui::CursorIcon::ResizeHorizontal,
        };
        ctx.set_cursor_icon(cursor);

        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
        }
    }

    /// Render the canvas right-click "Add Window..." popup, if open.
    fn quick_add_ui(&mut self, ctx: &egui::Context) {
        let Some(popup) = &mut self.quick_add else { return; };

        // Read this before drawing the popup: the focused search box's
        // TextEdit consumes the Escape key itself (to drop focus), so
        // checking afterwards would always see it as already consumed.
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut clicked_index = None;

        let area_response = egui::Area::new(egui::Id::new("quick_add_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(popup.screen_pos)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(22, 22, 26))
                    .rounding(8.0)
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 45, 52)))
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.set_width(240.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(egui_phosphor::regular::MAGNIFYING_GLASS)
                                    .size(13.0)
                                    .color(egui::Color32::from_rgb(140, 140, 150)),
                            );
                            ui.add_space(6.0);
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut popup.search)
                                    .desired_width(ui.available_width())
                                    .hint_text("Search windows...")
                                    .frame(false),
                            );
                            resp.request_focus();
                        });

                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(4.0);

                        let filter = popup.search.to_lowercase();
                        let matches = |w: &WindowInfo| {
                            filter.is_empty()
                                || w.title.to_lowercase().contains(&filter)
                                || w.exe_name.to_lowercase().contains(&filter)
                        };

                        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                            let mut any = false;
                            for (idx, window) in popup.windows.iter().enumerate() {
                                if !matches(window) {
                                    continue;
                                }
                                any = true;

                                let label = if window.title.is_empty() {
                                    &window.exe_name
                                } else {
                                    &window.title
                                };
                                let resp = ui.add_sized(
                                    Vec2::new(ui.available_width(), 22.0),
                                    egui::Button::new(egui::RichText::new(label).size(12.5))
                                        .frame(false),
                                );
                                if resp.clicked() {
                                    clicked_index = Some(idx);
                                }
                            }

                            if !any {
                                ui.add_space(8.0);
                                ui.label(
                                    egui::RichText::new("No matching windows")
                                        .size(11.5)
                                        .color(egui::Color32::from_rgb(120, 120, 128)),
                                );
                            }
                        });
                    });
            });

        if ctx.input(|i| i.pointer.any_click()) {
            if let Some(click_pos) = ctx.input(|i| i.pointer.interact_pos()) {
                if !area_response.response.rect.contains(click_pos) {
                    close = true;
                }
            }
        }

        if let Some(idx) = clicked_index {
            if let Some(popup) = &self.quick_add {
                if let Some(window) = popup.windows.get(idx) {
                    spawn_preview(
                        window,
                        &mut self.preview_manager,
                        &mut self.capture_coordinator,
                        popup.canvas_pos,
                        Vec2::new(320.0, 240.0),
                    );
                }
            }
            close = true;
        }

        if close {
            self.quick_add = None;
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

        layout.recent_browser_urls = self.recent_urls.clone();

        layout
    }

    /// Apply a SavedLayout to restore state
    fn apply_layout(&mut self, layout: &SavedLayout) {
        // Clear existing state
        self.preview_manager.clear();
        self.capture_coordinator.stop_all();
        #[cfg(windows)]
        self.browser.clear();

        // Restore canvas state
        self.canvas.pan = Vec2::new(layout.canvas.pan.0, layout.canvas.pan.1);
        self.canvas.zoom = layout.canvas.zoom;
        self.canvas.show_grid = layout.canvas.show_grid;

        self.recent_urls = layout.recent_browser_urls.clone();

        // Enumerate current windows to find matching ones
        let current_windows = enumerate_windows();

        // Restore previews
        for preview_layout in &layout.previews {
            // Browser tiles restore by recreating their WebView at the saved
            // URL; a failed host creation skips just this tile.
            #[cfg(windows)]
            if let Some(url) = &preview_layout.browser_url {
                match self.create_browser_tile(
                    url,
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    preview_layout.fps_preset,
                ) {
                    Ok(id) => {
                        self.preview_manager.set_z_order(id, preview_layout.z_order);
                        if let Some(preview) = self.preview_manager.get_mut(id) {
                            // Restored tiles appear instantly, no spawn animation.
                            preview.created_at = Instant::now() - Duration::from_secs(1);
                        }
                    }
                    Err(error) => {
                        log::error!("Failed to restore browser tile: {error}");
                    }
                }
                continue;
            }

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
                println!("Restored preview: {}", privacy::redact_title(&window_info.title));
            } else {
                #[cfg(debug_assertions)]
                println!("Window not found: {}", privacy::redact_title(&preview_layout.window_title));
            }
        }
    }
}

impl eframe::App for PluriviewApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Auto-save on exit
        self.save_autosave();
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Cache our window handle once; used by the tray and for positioning
        // browser host windows (no more FindWindowW-by-title every frame).
        if self.main_hwnd.is_none() {
            if let Ok(handle) = frame.window_handle() {
                if let RawWindowHandle::Win32(win32) = handle.as_raw() {
                    self.main_hwnd = Some(win32.hwnd.get());
                }
            }
        }

        // Set up tray HWND on first frame (window now exists)
        self.setup_tray_hwnd();

        // Custom title bar + manual resize border (decorations are off)
        self.handle_frameless_resize(ctx);
        self.title_bar_ui(ctx);

        // Process any pending captured frames
        self.capture_coordinator.process_frames(&mut self.preview_manager, ctx);

        // Handle pending region selection request (from context menu in canvas)
        if let Some(preview_id) = self.canvas.pending_region_select.take() {
            if let Some(preview) = self.preview_manager.get(preview_id) {
                if let Some(ref handle) = preview.window_handle {
                    // Start the region selector overlay
                    if let Some(selector) = RegionSelector::show_for_window(handle.hwnd) {
                        self.region_selector = Some(selector);
                        self.region_select_preview_id = Some(preview_id);
                    }
                }
            }
        }

        // Poll for region selection result
        if let Some(ref mut selector) = self.region_selector {
            if let Some(result) = selector.poll_result() {
                if let Some(selection) = result {
                    // Apply the crop to the preview
                    if let Some(preview_id) = self.region_select_preview_id {
                        if let Some(preview) = self.preview_manager.get_mut(preview_id) {
                            // Get source dimensions from frame if available
                            if let Some((w, h)) = preview.frame_size {
                                let crop_uv = selection.to_uv(w, h);
                                preview.crop_uv = Some(crop_uv);
                                // Update aspect ratio for the cropped region
                                let crop_w = (crop_uv.2 - crop_uv.0) * w as f32;
                                let crop_h = (crop_uv.3 - crop_uv.1) * h as f32;
                                if crop_h > 0.0 {
                                    preview.source_aspect_ratio = crop_w / crop_h;
                                }
                            }
                        }
                    }
                }
                // Clear the selector (whether successful or cancelled)
                self.region_selector = None;
                self.region_select_preview_id = None;
            }
        }

        // Menu bar (File / View / Help) now lives inline in the custom
        // title bar; see `title_bar_ui` / `menu_bar`.

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

        #[cfg(windows)]
        {
            let browser_double_clicked = self
                .canvas
                .last_double_clicked
                .filter(|id| self.browser.contains(*id));

            let browser_shortcut = (!ctx.wants_keyboard_input()
                && ctx.input(|input| input.modifiers.ctrl && input.key_pressed(egui::Key::B)))
            .then(|| {
                self.canvas
                    .selection
                    .iter()
                    .copied()
                    .find(|id| self.browser.contains(*id))
            })
            .flatten();

            if let Some(id) = browser_double_clicked.or(browser_shortcut) {
                let active = self.browser.get(id).is_some_and(|host| host.is_active());
                if active {
                    if let Some(host) = self.browser.get_mut(id) {
                        host.park();
                    }
                    self.browser_activated_at = None;
                } else if let (Some(hwnd), Some(canvas_rect)) =
                    (self.main_hwnd, self.canvas.last_screen_rect)
                {
                    if let Some(rect) = self.browser_tile_rect(id, canvas_rect) {
                        self.browser.park_all();
                        // Bring to front + select so the accent outline shows
                        // around the live window's inset edge.
                        self.preview_manager.bring_to_front(id);
                        self.canvas.selection = vec![id];
                        if let Some(host) = self.browser.get_mut(id) {
                            host.place(
                                HWND(hwnd as *mut _),
                                rect,
                                ctx.pixels_per_point(),
                                true,
                            );
                        }
                        self.browser_activated_at = Some(Instant::now());
                    }
                }
            }
            self.canvas.last_double_clicked = None;

            // Per-frame browser housekeeping: mirror page titles/URLs onto
            // tiles, exit interaction mode on Escape/focus loss/minimize,
            // and keep the live host glued to its tile through pan/zoom
            // and window moves.
            self.browser_frame(ctx);
        }

        // Canvas right-click "Add Window..." was selected: open the
        // quick-add popup at that spot with a fresh window snapshot.
        if let Some((canvas_pos, screen_pos)) = self.canvas.pending_quick_add.take() {
            self.quick_add = Some(QuickAddPopup {
                canvas_pos,
                screen_pos,
                windows: enumerate_windows(),
                search: String::new(),
            });
        }

        #[cfg(windows)]
        {
            if let Some(position) = self.canvas.pending_browser_add.take() {
                self.add_browser = Some(AddBrowserDialog {
                    position,
                    url: String::new(),
                    error: None,
                    target: None,
                    focused: false,
                });
            }

            // Actions queued by browser tile hover controls / context menus.
            for (id, action) in std::mem::take(&mut self.canvas.pending_browser_actions) {
                self.handle_browser_action(ctx, id, action);
            }

            // "Undo" on a removed browser tile: recreate the WebView from
            // its saved URL (the original host window is already destroyed).
            if let Some(info) = self.canvas.pending_browser_restore.take() {
                if let Some(url) = info.browser_url.clone() {
                    if let Err(error) =
                        self.create_browser_tile(&url, info.position, info.size, info.fps_preset)
                    {
                        log::error!("Failed to restore browser tile: {error}");
                    }
                }
            }
        }

        self.quick_add_ui(ctx);
        #[cfg(windows)]
        self.add_browser_ui(ctx);

        #[cfg(windows)]
        {
            let previews = &self.preview_manager;
            self.browser.retain(|id| previews.get(id).is_some());
        }

        // Handle global keyboard shortcuts (skip while typing in a text field)
        if !ctx.wants_keyboard_input() {
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
        }

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
                        ui.label("Version 0.2");
                        ui.add_space(10.0);
                        ui.label("Live window preview application");
                        ui.label("with infinite canvas");
                        ui.add_space(15.0);
                        ui.label(egui::RichText::new("Created by Spatpit").weak());
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

                            ui.label(egui::RichText::new("Browser Tiles").strong());
                            ui.label("");
                            ui.end_row();

                            ui.label("Add browser");
                            ui.label(egui::RichText::new("Right-click canvas").weak());
                            ui.end_row();

                            ui.label("Interact with page");
                            ui.label(egui::RichText::new("Double-click / Ctrl+B").weak());
                            ui.end_row();

                            ui.label("Exit interaction");
                            ui.label(egui::RichText::new("Esc / click outside").weak());
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

        // Schedule the next repaint instead of spinning at uncapped frame rate.
        // When a capture is live we refresh at ~60 FPS so previews stay smooth;
        // otherwise we tick slowly, which is still frequent enough to process
        // tray events while keeping the app near-idle on the CPU.
        // (egui repaints immediately on input regardless of this hint.)
        let repaint_after = if self.capture_coordinator.has_live_capture() {
            std::time::Duration::from_millis(16)
        } else {
            std::time::Duration::from_millis(250)
        };
        ctx.request_repaint_after(repaint_after);
    }
}
