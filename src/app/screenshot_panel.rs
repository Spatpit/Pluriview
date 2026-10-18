use super::*;
use crate::screenshot_library::{self, ScreenshotFolder};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::SystemTime;

/// Folder contents can change in Explorer; refresh while the panel is open.
const RESCAN_INTERVAL: Duration = Duration::from_secs(5);
const THUMBNAIL_DECODE_SIDE: u32 = 320;
const MAX_THUMBNAILS_IN_FLIGHT: usize = 4;
const MAX_CACHED_THUMBNAILS: usize = 240;
const FRESH_SECS: f32 = 1.4;
const CARD_GAP: f32 = 8.0;

/// A screenshot being dragged out of the panel.
#[derive(Clone, Debug)]
pub(super) struct ShotDrag {
    pub path: PathBuf,
}

#[derive(Default)]
pub(super) struct PanelState {
    folders: Vec<ScreenshotFolder>,
    scan: Option<mpsc::Receiver<Vec<ScreenshotFolder>>>,
    scan_queued: bool,
    last_scan: Option<Instant>,
    thumbnails: HashMap<PathBuf, Thumbnail>,
    worker: Option<ThumbnailWorker>,
    in_flight: usize,
    frame: u64,
    collapsed: HashSet<Option<String>>,
    dialog: Option<FolderDialog>,
    /// Header of the folder new screenshots go to, while visible.
    pub(super) landing_rect: Option<egui::Rect>,
    fresh: Option<(PathBuf, Instant)>,
}

enum Thumbnail {
    Loading,
    Ready {
        texture: egui::TextureHandle,
        modified: SystemTime,
        used: u64,
    },
    Failed {
        modified: SystemTime,
    },
}

struct ThumbnailWorker {
    requests: mpsc::Sender<(PathBuf, SystemTime)>,
    results: mpsc::Receiver<(PathBuf, SystemTime, Option<egui::ColorImage>)>,
}

impl ThumbnailWorker {
    fn start() -> Option<Self> {
        let (requests, request_receiver) = mpsc::channel::<(PathBuf, SystemTime)>();
        let (result_sender, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("pluriview-screenshot-thumbnails".to_owned())
            .spawn(move || {
                // Ends when the panel state (and its sender) is dropped.
                while let Ok((path, modified)) = request_receiver.recv() {
                    let image = image::open(&path).ok().map(|image| {
                        let image = image
                            .thumbnail(THUMBNAIL_DECODE_SIDE, THUMBNAIL_DECODE_SIDE)
                            .to_rgba8();
                        let size = [image.width() as usize, image.height() as usize];
                        egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw())
                    });
                    if result_sender.send((path, modified, image)).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self { requests, results })
    }
}

struct FolderDialog {
    /// Folder being renamed; `None` creates a new folder.
    rename: Option<String>,
    name: String,
    error: Option<String>,
    focus: bool,
}

enum PanelAction {
    SaveTo(Option<String>),
    NewFolder,
    RenameFolder(String),
    Move(PathBuf, Option<String>),
    Delete(PathBuf),
    OpenShot(PathBuf),
    ShowShot(PathBuf),
    OpenFolder(Option<String>),
}

fn folder_label(name: Option<&str>) -> &str {
    name.unwrap_or("Unsorted")
}

impl PanelState {
    pub(super) fn request_scan(&mut self) {
        self.scan_queued = true;
    }

    /// Reuse the flight thumbnail so a fresh screenshot appears at once.
    pub(super) fn add_saved_shot(&mut self, path: PathBuf, texture: Option<egui::TextureHandle>) {
        if let Some(texture) = texture {
            let modified = std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            self.thumbnails.insert(
                path.clone(),
                Thumbnail::Ready {
                    texture,
                    modified,
                    used: self.frame,
                },
            );
        }
        self.fresh = Some((path, Instant::now()));
        self.scan_queued = true;
    }

    fn upkeep(&mut self, root: &Path, ctx: &egui::Context) {
        self.frame += 1;
        if let Some(receiver) = &self.scan {
            match receiver.try_recv() {
                Ok(folders) => {
                    self.folders = folders;
                    self.scan = None;
                    let existing: HashSet<&Path> = self
                        .folders
                        .iter()
                        .flat_map(|folder| folder.shots.iter().map(|shot| shot.path.as_path()))
                        .collect();
                    self.thumbnails.retain(|path, thumbnail| {
                        existing.contains(path.as_path()) || matches!(thumbnail, Thumbnail::Loading)
                    });
                }
                Err(mpsc::TryRecvError::Disconnected) => self.scan = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let due = self
            .last_scan
            .is_none_or(|last| last.elapsed() >= RESCAN_INTERVAL);
        if self.scan.is_none() && (self.scan_queued || due) {
            self.scan_queued = false;
            self.last_scan = Some(Instant::now());
            let (sender, receiver) = mpsc::channel();
            let root = root.to_path_buf();
            let spawned = std::thread::Builder::new()
                .name("pluriview-screenshot-scan".to_owned())
                .spawn(move || {
                    let _ = sender.send(screenshot_library::scan(&root));
                });
            if spawned.is_ok() {
                self.scan = Some(receiver);
            }
        }

        if let Some(worker) = &self.worker {
            while let Ok((path, modified, image)) = worker.results.try_recv() {
                self.in_flight = self.in_flight.saturating_sub(1);
                let thumbnail = match image {
                    Some(image) => Thumbnail::Ready {
                        texture: ctx.load_texture(
                            format!("screenshot_thumb_{}", path.display()),
                            image,
                            egui::TextureOptions::LINEAR,
                        ),
                        modified,
                        used: self.frame,
                    },
                    None => Thumbnail::Failed { modified },
                };
                self.thumbnails.insert(path, thumbnail);
            }
        }
        if self.thumbnails.len() > MAX_CACHED_THUMBNAILS {
            let mut ready: Vec<(u64, PathBuf)> = self
                .thumbnails
                .iter()
                .filter_map(|(path, thumbnail)| match thumbnail {
                    Thumbnail::Ready { used, .. } => Some((*used, path.clone())),
                    _ => None,
                })
                .collect();
            ready.sort();
            let excess = self.thumbnails.len() - MAX_CACHED_THUMBNAILS;
            for (_, path) in ready.into_iter().take(excess) {
                self.thumbnails.remove(&path);
            }
        }
        if self.scan.is_some() || self.in_flight > 0 {
            ctx.request_repaint_after(Duration::from_millis(50));
        } else {
            ctx.request_repaint_after(RESCAN_INTERVAL);
        }
    }

    /// Texture for a visible card, asking the worker to decode it if needed.
    fn thumbnail(
        &mut self,
        path: &Path,
        modified: SystemTime,
    ) -> Option<(egui::TextureId, egui::Vec2)> {
        match self.thumbnails.get_mut(path) {
            Some(Thumbnail::Ready {
                texture,
                modified: loaded,
                used,
            }) if *loaded == modified => {
                *used = self.frame;
                return Some((texture.id(), texture.size_vec2()));
            }
            Some(Thumbnail::Failed { modified: loaded }) if *loaded == modified => return None,
            Some(Thumbnail::Loading) => return None,
            _ => {}
        }
        if self.in_flight >= MAX_THUMBNAILS_IN_FLIGHT {
            return None;
        }
        if self.worker.is_none() {
            self.worker = ThumbnailWorker::start();
        }
        let worker = self.worker.as_ref()?;
        if worker.requests.send((path.to_path_buf(), modified)).is_ok() {
            self.in_flight += 1;
            self.thumbnails
                .insert(path.to_path_buf(), Thumbnail::Loading);
        }
        None
    }

    fn texture_for(&self, path: &Path) -> Option<egui::TextureId> {
        match self.thumbnails.get(path) {
            Some(Thumbnail::Ready { texture, .. }) => Some(texture.id()),
            _ => None,
        }
    }
}

#[cfg(windows)]
fn move_to_recycle_bin(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE,
        SHFILEOPSTRUCTW,
    };
    // pFrom is a list of paths ending with an extra null.
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    let result = unsafe { SHFileOperationW(&mut operation) };
    if result == 0 && !operation.fAnyOperationsAborted.as_bool() {
        Ok(())
    } else {
        Err("Could not move the screenshot to the Recycle Bin.".to_owned())
    }
}

#[cfg(not(windows))]
fn move_to_recycle_bin(_path: &Path) -> Result<(), String> {
    Err("Deleting screenshots is only supported on Windows.".to_owned())
}

#[cfg(windows)]
fn shell_open(file: &std::ffi::OsStr, parameters: Option<&std::ffi::OsStr>) {
    let file = HSTRING::from(file);
    let parameters = parameters.map(HSTRING::from);
    unsafe {
        match &parameters {
            Some(parameters) => ShellExecuteW(
                None,
                windows::core::w!("open"),
                &file,
                parameters,
                None,
                SW_SHOWNORMAL,
            ),
            None => ShellExecuteW(
                None,
                windows::core::w!("open"),
                &file,
                None,
                None,
                SW_SHOWNORMAL,
            ),
        };
    }
}

impl PluriviewApp {
    pub(super) fn set_screenshots_panel_open(&mut self, open: bool) {
        if self.app_config.screenshots_panel_open == open {
            return;
        }
        self.app_config.screenshots_panel_open = open;
        if open {
            self.screenshots.panel.request_scan();
        }
        self.save_app_config();
    }

    /// Folder new screenshots are saved to. A folder removed outside
    /// Pluriview sends new shots back to the top level.
    pub(super) fn screenshot_save_dir(&mut self) -> Option<PathBuf> {
        let root = self.screenshots_dir()?;
        let folder = self.app_config.screenshot_folder.clone();
        let directory = screenshot_library::folder_path(&root, folder.as_deref());
        if folder.is_some() && (directory == root || !directory.is_dir()) {
            self.app_config.screenshot_folder = None;
            self.save_app_config();
            return Some(root);
        }
        Some(directory)
    }

    pub(super) fn screenshots_panel_ui(&mut self, ctx: &egui::Context) {
        self.screenshots.panel.landing_rect = None;
        if !self.app_config.screenshots_panel_open || self.canvas_only {
            return;
        }
        let Some(root) = self.screenshots_dir() else {
            return;
        };
        self.screenshots.panel.upkeep(&root, ctx);
        let save_folder = self.app_config.screenshot_folder.clone();
        let panel_shortcut = self
            .app_config
            .keyboard_shortcuts
            .get(HotkeySlot::ToggleScreenshotsPanel);
        let shot_shortcut = self
            .app_config
            .keyboard_shortcuts
            .get(HotkeySlot::ScreenshotTile);
        let mut actions = Vec::new();
        let mut close = false;

        egui::SidePanel::right("screenshots_panel")
            .default_width(292.0)
            .min_width(240.0)
            .max_width(420.0)
            .frame(
                egui::Frame::none()
                    .fill(crate::ui_theme::PANEL)
                    .inner_margin(egui::Margin::same(12.0)),
            )
            .show(ctx, |ui| {
                let state = &mut self.screenshots.panel;
                ui.add_space(4.0);
                let title = ui
                    .horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Screenshots")
                                .size(19.0)
                                .strong()
                                .color(crate::ui_theme::TEXT),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let icon = |icon: &str| {
                                egui::Button::new(egui::RichText::new(icon).size(15.0))
                                    .frame(false)
                                    .min_size(egui::vec2(26.0, 24.0))
                            };
                            if ui
                                .add(icon(egui_phosphor::regular::X))
                                .on_hover_text(format!(
                                    "Hide Screenshots ({})",
                                    panel_shortcut.display()
                                ))
                                .clicked()
                            {
                                close = true;
                            }
                            if ui
                                .add(icon(egui_phosphor::regular::FOLDER_OPEN))
                                .on_hover_text("Open in Explorer")
                                .clicked()
                            {
                                actions.push(PanelAction::OpenFolder(None));
                            }
                            if ui
                                .add(icon(egui_phosphor::regular::FOLDER_PLUS))
                                .on_hover_text("New folder")
                                .clicked()
                            {
                                actions.push(PanelAction::NewFolder);
                            }
                        });
                    })
                    .response
                    .rect;
                ui.label(
                    egui::RichText::new("Drag a screenshot onto a folder to file it")
                        .size(12.0)
                        .color(crate::ui_theme::SECONDARY),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Save new shots to")
                            .size(12.5)
                            .color(crate::ui_theme::SECONDARY),
                    );
                    egui::ComboBox::from_id_salt("screenshot_save_folder")
                        .selected_text(folder_label(save_folder.as_deref()))
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for folder in &state.folders {
                                let selected = folder.name == save_folder;
                                if ui
                                    .selectable_label(selected, folder_label(folder.name.as_deref()))
                                    .clicked()
                                    && !selected
                                {
                                    actions.push(PanelAction::SaveTo(folder.name.clone()));
                                }
                            }
                        });
                });
                ui.add_space(8.0);

                let dragging = egui::DragAndDrop::payload::<ShotDrag>(ui.ctx());
                let released = ui.input(|input| input.pointer.any_released());
                let mut landing = None;
                let total: usize = state.folders.iter().map(|folder| folder.shots.len()).sum();

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if total == 0 && state.folders.len() <= 1 {
                            ui.add_space(24.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new(egui_phosphor::regular::CAMERA)
                                        .size(28.0)
                                        .color(crate::ui_theme::SECONDARY),
                                );
                                ui.label(
                                    egui::RichText::new("No screenshots yet")
                                        .strong()
                                        .color(crate::ui_theme::TEXT),
                                );
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Select a tile and press {}, or use the camera in the top bar.",
                                        shot_shortcut.display()
                                    ))
                                    .size(12.0)
                                    .color(crate::ui_theme::SECONDARY),
                                );
                            });
                        }
                        let folders = state.folders.clone();
                        for folder in &folders {
                            let section_top = ui.cursor().top();
                            let header = folder_header(
                                ui,
                                folder,
                                folder.name == save_folder,
                                !state.collapsed.contains(&folder.name),
                                dragging.is_some(),
                            );
                            if folder.name == save_folder && ui.is_rect_visible(header.rect) {
                                landing = Some(header.rect);
                            }
                            if header.clicked() && !state.collapsed.remove(&folder.name) {
                                state.collapsed.insert(folder.name.clone());
                            }
                            header.context_menu(|ui| {
                                crate::ui_theme::menu(ui);
                                if folder.name != save_folder
                                    && ui.button("Save New Screenshots Here").clicked()
                                {
                                    actions.push(PanelAction::SaveTo(folder.name.clone()));
                                    ui.close_menu();
                                }
                                if let Some(name) = &folder.name {
                                    if ui.button("Rename...").clicked() {
                                        actions.push(PanelAction::RenameFolder(name.clone()));
                                        ui.close_menu();
                                    }
                                }
                                if ui.button("Open in Explorer").clicked() {
                                    actions.push(PanelAction::OpenFolder(folder.name.clone()));
                                    ui.close_menu();
                                }
                            });

                            if !state.collapsed.contains(&folder.name) {
                                shot_grid(ui, state, folder, &folders, &mut actions);
                            }
                            ui.add_space(6.0);

                            // The whole section, header and cards, accepts a drop.
                            let section = egui::Rect::from_min_max(
                                egui::pos2(ui.min_rect().left(), section_top),
                                egui::pos2(ui.min_rect().right(), ui.cursor().top()),
                            );
                            if let Some(payload) = dragging.as_ref() {
                                if ui.rect_contains_pointer(section) {
                                    ui.painter().rect_stroke(
                                        section.shrink(1.0),
                                        8.0,
                                        egui::Stroke::new(1.5, crate::ui_theme::GOLD),
                                    );
                                    if released {
                                        actions.push(PanelAction::Move(
                                            payload.path.clone(),
                                            folder.name.clone(),
                                        ));
                                    }
                                }
                            }
                        }
                    });
                state.landing_rect = landing.or(Some(title));
            });

        if let Some(payload) = egui::DragAndDrop::payload::<ShotDrag>(ctx) {
            if let (Some(pointer), Some(texture)) = (
                ctx.pointer_latest_pos(),
                self.screenshots.panel.texture_for(&payload.path),
            ) {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip,
                    egui::Id::new("screenshot_drag_ghost"),
                ));
                let rect = egui::Rect::from_min_size(
                    pointer + egui::vec2(10.0, 10.0),
                    egui::vec2(112.0, 63.0),
                );
                painter.rect_filled(
                    rect.translate(egui::vec2(0.0, 3.0)),
                    6.0,
                    egui::Color32::from_black_alpha(90),
                );
                painter.image(
                    texture,
                    rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE.gamma_multiply(0.85),
                );
            }
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }

        if close {
            self.set_screenshots_panel_open(false);
        }
        for action in actions {
            self.handle_screenshot_panel_action(action, &root);
        }
        self.folder_dialog_ui(ctx, &root);
    }

    fn handle_screenshot_panel_action(&mut self, action: PanelAction, root: &Path) {
        let mut notice = None;
        match action {
            PanelAction::SaveTo(folder) => {
                self.app_config.screenshot_folder = folder;
                self.save_app_config();
            }
            PanelAction::NewFolder => {
                self.screenshots.panel.dialog = Some(FolderDialog {
                    rename: None,
                    name: String::new(),
                    error: None,
                    focus: true,
                });
            }
            PanelAction::RenameFolder(name) => {
                self.screenshots.panel.dialog = Some(FolderDialog {
                    rename: Some(name.clone()),
                    name,
                    error: None,
                    focus: true,
                });
            }
            PanelAction::Move(path, folder) => {
                match screenshot_library::move_shot(root, &path, folder.as_deref()) {
                    Ok(moved) => {
                        if moved != path {
                            if let Some(thumbnail) = self.screenshots.panel.thumbnails.remove(&path)
                            {
                                self.screenshots
                                    .panel
                                    .thumbnails
                                    .insert(moved.clone(), thumbnail);
                            }
                            self.screenshots.panel.fresh = Some((moved, Instant::now()));
                        }
                    }
                    Err(error) => notice = Some(error),
                }
                self.screenshots.panel.request_scan();
            }
            PanelAction::Delete(path) => {
                if screenshot_library::shot_folder(root, &path).is_some() {
                    if let Err(error) = move_to_recycle_bin(&path) {
                        notice = Some(error);
                    }
                }
                self.screenshots.panel.request_scan();
            }
            PanelAction::OpenShot(path) => {
                #[cfg(windows)]
                if screenshot_library::shot_folder(root, &path).is_some() {
                    shell_open(path.as_os_str(), None);
                }
            }
            PanelAction::ShowShot(path) => {
                #[cfg(windows)]
                if screenshot_library::shot_folder(root, &path).is_some() {
                    let mut select = std::ffi::OsString::from("/select,\"");
                    select.push(path.as_os_str());
                    select.push("\"");
                    shell_open(std::ffi::OsStr::new("explorer.exe"), Some(&select));
                }
            }
            PanelAction::OpenFolder(folder) => {
                let directory = screenshot_library::folder_path(root, folder.as_deref());
                match std::fs::create_dir_all(&directory) {
                    #[cfg(windows)]
                    Ok(()) => shell_open(directory.as_os_str(), None),
                    #[cfg(not(windows))]
                    Ok(()) => {}
                    Err(error) => {
                        notice = Some(format!("Could not open the folder: {error}"));
                    }
                }
            }
        }
        if let Some(message) = notice {
            self.canvas_notice = Some((message, Instant::now()));
        }
    }

    fn folder_dialog_ui(&mut self, ctx: &egui::Context, root: &Path) {
        let Some(dialog) = self.screenshots.panel.dialog.as_mut() else {
            return;
        };
        let title = if dialog.rename.is_some() {
            "Rename Folder"
        } else {
            "New Folder"
        };
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut dialog.name)
                        .hint_text("Folder name")
                        .desired_width(240.0),
                );
                if dialog.focus {
                    response.request_focus();
                    dialog.focus = false;
                }
                if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                if let Some(error) = &dialog.error {
                    ui.label(egui::RichText::new(error).color(crate::ui_theme::DANGER));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    let label = if dialog.rename.is_some() {
                        "Rename"
                    } else {
                        "Create"
                    };
                    if ui.button(label).clicked() {
                        submit = true;
                    }
                });
            });
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        if cancel {
            self.screenshots.panel.dialog = None;
            return;
        }
        if !submit {
            return;
        }
        let rename = dialog.rename.clone();
        let name = dialog.name.clone();
        let result = match &rename {
            Some(old) => screenshot_library::rename_folder(root, old, &name),
            None => screenshot_library::create_folder(root, &name),
        };
        match result {
            Ok(name) => {
                self.screenshots.panel.dialog = None;
                self.screenshots.panel.request_scan();
                let panel = &mut self.screenshots.panel;
                if let Some(old) = &rename {
                    if panel.collapsed.remove(&Some(old.clone())) {
                        panel.collapsed.insert(Some(name.clone()));
                    }
                    // Thumbnails are keyed by path; the scan reloads them.
                    panel.thumbnails.clear();
                    if self.app_config.screenshot_folder.as_ref() == Some(old) {
                        self.app_config.screenshot_folder = Some(name);
                        self.save_app_config();
                    }
                } else {
                    // A new folder is where the next screenshots go.
                    self.app_config.screenshot_folder = Some(name);
                    self.save_app_config();
                }
            }
            Err(error) => {
                if let Some(dialog) = self.screenshots.panel.dialog.as_mut() {
                    dialog.error = Some(error);
                }
            }
        }
    }
}

fn folder_header(
    ui: &mut egui::Ui,
    folder: &ScreenshotFolder,
    saving_here: bool,
    expanded: bool,
    dragging: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::click());
    let fill = if response.hovered() && !dragging {
        crate::ui_theme::HOVER
    } else {
        crate::ui_theme::SURFACE
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 7.0, fill);
    if saving_here {
        painter.rect_stroke(
            rect,
            7.0,
            egui::Stroke::new(1.0, crate::ui_theme::GOLD_DARK),
        );
    }
    let caret = if expanded {
        egui_phosphor::regular::CARET_DOWN
    } else {
        egui_phosphor::regular::CARET_RIGHT
    };
    let icon = if folder.name.is_some() {
        egui_phosphor::regular::FOLDER_SIMPLE
    } else {
        egui_phosphor::regular::TRAY
    };
    let secondary = crate::ui_theme::SECONDARY;
    painter.text(
        rect.left_center() + egui::vec2(10.0, 0.0),
        egui::Align2::LEFT_CENTER,
        caret,
        egui::FontId::proportional(12.0),
        secondary,
    );
    painter.text(
        rect.left_center() + egui::vec2(27.0, 0.0),
        egui::Align2::LEFT_CENTER,
        icon,
        egui::FontId::proportional(15.0),
        if saving_here {
            crate::ui_theme::GOLD
        } else {
            secondary
        },
    );
    let count = folder.shots.len().to_string();
    painter.text(
        rect.right_center() - egui::vec2(10.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        &count,
        egui::FontId::proportional(12.0),
        secondary,
    );
    let name_rect = egui::Rect::from_min_max(
        rect.min + egui::vec2(48.0, 0.0),
        rect.max - egui::vec2(36.0, 0.0),
    );
    crate::ui_theme::clipped_text(
        ui,
        name_rect,
        folder_label(folder.name.as_deref()),
        13.5,
        crate::ui_theme::TEXT,
    );
    let response = if saving_here {
        response.on_hover_text("New screenshots are saved here")
    } else {
        response
    };
    ui.add_space(4.0);
    response
}

fn shot_grid(
    ui: &mut egui::Ui,
    state: &mut PanelState,
    folder: &ScreenshotFolder,
    folders: &[ScreenshotFolder],
    actions: &mut Vec<PanelAction>,
) {
    if folder.shots.is_empty() {
        ui.label(
            egui::RichText::new("Empty. Drop screenshots here.")
                .size(12.0)
                .color(crate::ui_theme::SECONDARY),
        );
        return;
    }
    let width = ui.available_width();
    let columns = if width >= 220.0 { 2 } else { 1 };
    let card_width = (width - CARD_GAP * (columns - 1) as f32) / columns as f32;
    let image_height = card_width * 9.0 / 16.0;
    let card_size = egui::vec2(card_width, image_height + 20.0);
    let fresh = state
        .fresh
        .as_ref()
        .filter(|(_, started)| started.elapsed().as_secs_f32() < FRESH_SECS)
        .map(|(path, started)| (path.clone(), started.elapsed().as_secs_f32() / FRESH_SECS));
    if fresh.is_some() {
        ui.ctx().request_repaint();
    }

    for row in folder.shots.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for shot in row {
                let id = egui::Id::new(("screenshot_card", &shot.path));
                // A path-based id keeps a drag alive while the grid re-sorts.
                let (rect, _) = ui.allocate_exact_size(card_size, egui::Sense::hover());
                let response = ui.interact(rect, id, egui::Sense::click_and_drag());
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let image_rect =
                    egui::Rect::from_min_size(rect.min, egui::vec2(card_width, image_height));
                let painter = ui.painter();
                painter.rect_filled(image_rect, 6.0, crate::ui_theme::SURFACE);
                if let Some((texture, size)) = state.thumbnail(&shot.path, shot.modified) {
                    let fit = (image_rect.width() / size.x).min(image_rect.height() / size.y);
                    let fitted = egui::Rect::from_center_size(image_rect.center(), size * fit);
                    painter.image(
                        texture,
                        fitted,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                } else {
                    painter.text(
                        image_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        egui_phosphor::regular::IMAGE,
                        egui::FontId::proportional(18.0),
                        crate::ui_theme::SECONDARY,
                    );
                }
                let highlight = if let Some((path, t)) = &fresh {
                    (path == &shot.path).then(|| (1.0 - t).clamp(0.0, 1.0))
                } else {
                    None
                };
                if let Some(alpha) = highlight {
                    painter.rect_stroke(
                        image_rect.expand(1.0),
                        7.0,
                        egui::Stroke::new(2.0, crate::ui_theme::GOLD.gamma_multiply(alpha)),
                    );
                } else if response.hovered() {
                    painter.rect_stroke(
                        image_rect,
                        6.0,
                        egui::Stroke::new(1.0, crate::ui_theme::BORDER),
                    );
                }
                let label_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 2.0, image_rect.bottom() + 2.0),
                    rect.max,
                );
                crate::ui_theme::clipped_text(
                    ui,
                    label_rect,
                    &shot.name,
                    11.5,
                    crate::ui_theme::SECONDARY,
                );

                if response.drag_started() {
                    egui::DragAndDrop::set_payload(
                        ui.ctx(),
                        ShotDrag {
                            path: shot.path.clone(),
                        },
                    );
                }
                if response.double_clicked() {
                    actions.push(PanelAction::OpenShot(shot.path.clone()));
                }
                let response = response.on_hover_text(&shot.name);
                response.context_menu(|ui| {
                    crate::ui_theme::menu(ui);
                    if ui.button("Open").clicked() {
                        actions.push(PanelAction::OpenShot(shot.path.clone()));
                        ui.close_menu();
                    }
                    if ui.button("Show in Explorer").clicked() {
                        actions.push(PanelAction::ShowShot(shot.path.clone()));
                        ui.close_menu();
                    }
                    ui.menu_button("Move to", |ui| {
                        crate::ui_theme::menu(ui);
                        for target in folders {
                            if target.name == folder.name {
                                continue;
                            }
                            if ui.button(folder_label(target.name.as_deref())).clicked() {
                                actions.push(PanelAction::Move(
                                    shot.path.clone(),
                                    target.name.clone(),
                                ));
                                ui.close_menu();
                            }
                        }
                        if ui.button("New Folder...").clicked() {
                            actions.push(PanelAction::NewFolder);
                            ui.close_menu();
                        }
                    });
                    ui.separator();
                    if crate::ui_theme::destructive(ui, "Move to Recycle Bin").clicked() {
                        actions.push(PanelAction::Delete(shot.path.clone()));
                        ui.close_menu();
                    }
                });
            }
        });
        ui.add_space(CARD_GAP);
    }
}
