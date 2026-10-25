use super::*;
use crate::screenshot::{self, CapturedPixels, CombinedPart, LocalTimestamp, ScreenshotImage};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

/// How long a live capture may take to deliver its full-size frame before
/// the screenshot settles for the frame already on screen.
const NATIVE_FRAME_WAIT: Duration = Duration::from_millis(700);
/// libmpv readbacks normally finish on the next paint.
const VIDEO_SNAPSHOT_WAIT: Duration = Duration::from_secs(1);
const FLASH_SECS: f32 = 0.28;
const FLIGHT_SECS: f32 = 0.6;
const PULSE_SECS: f32 = 0.4;
pub(super) const THUMBNAIL_SIDE: u32 = 240;
const LANDED_SIDE: f32 = 26.0;

/// Runtime-only screenshot work and its animations.
#[derive(Default)]
pub(super) struct ScreenshotState {
    pending: Vec<PendingShot>,
    pending_combined: Vec<PendingCombined>,
    saving: Vec<SavingShot>,
    flashes: Vec<(egui::Rect, Instant)>,
    flights: Vec<Flight>,
    /// Where the Capture button was drawn this frame; flights land there.
    pub(super) button_rect: Option<egui::Rect>,
    /// Where the last flight landed, pulsing briefly.
    pulse: Option<(Instant, egui::Rect)>,
    next_thumbnail: u64,
    pub(super) panel: super::screenshot_panel::PanelState,
    pub(super) landings: Vec<super::screenshot_drop::Landing>,
}

struct SavingShot {
    receiver: mpsc::Receiver<Result<PathBuf, String>>,
    /// Reused as the panel thumbnail once the file exists.
    thumbnail: Option<egui::TextureHandle>,
}

struct PendingShot {
    id: PreviewId,
    stem: String,
    crop_uv: Option<(f32, f32, f32, f32)>,
    requested_at: Instant,
    source: PendingSource,
}

/// Several selected tiles saved as one image once each has its frame.
struct PendingCombined {
    stem: String,
    requested_at: Instant,
    /// Bottom to top, as the canvas paints them.
    tiles: Vec<CombinedTile>,
}

struct CombinedTile {
    id: PreviewId,
    crop_uv: Option<(f32, f32, f32, f32)>,
    /// Where the tile was on screen when the shot was taken.
    rect: egui::Rect,
    frame: CombinedFrame,
}

enum CombinedFrame {
    Ready(CapturedPixels),
    Pending(PendingSource),
    /// The tile had nothing to show; the image leaves it out.
    Missing,
}

enum PendingSource {
    /// Rereading a still image whose decoded pixels were released.
    ImageFile(mpsc::Receiver<Option<ScreenshotImage>>),
    /// Waiting for a live capture to deliver one frame at its full size.
    NativeFrame {
        fallback: Option<Arc<egui::ColorImage>>,
    },
    #[cfg(windows)]
    Video(mpsc::Receiver<Option<VideoSnapshot>>),
}

struct Flight {
    from: egui::Rect,
    texture: egui::TextureHandle,
    started: Instant,
}

enum ShotStart {
    Ready(CapturedPixels),
    Pending(PendingSource),
    Unavailable(&'static str),
}

/// `None` while the tile's frame is still on its way, then the frame, or
/// `Some(None)` when there is none to be had.
fn poll_source(
    source: &mut PendingSource,
    preview: Option<&mut Preview>,
    requested_at: Instant,
) -> Option<Option<CapturedPixels>> {
    let expired = |wait: Duration| requested_at.elapsed() >= wait;
    match (source, preview) {
        (_, None) => Some(None),
        (PendingSource::ImageFile(receiver), Some(_)) => match receiver.try_recv() {
            Ok(image) => Some(image.map(CapturedPixels::Straight)),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(None),
        },
        (PendingSource::NativeFrame { fallback }, Some(preview)) => {
            if let Some(frame) = preview.take_screenshot_native_frame() {
                Some(Some(CapturedPixels::Premultiplied(frame)))
            } else if expired(NATIVE_FRAME_WAIT) {
                preview.cancel_screenshot_native_frame();
                fallback
                    .take()
                    .or_else(|| preview.latest_capture_frame())
                    .map(|frame| Some(CapturedPixels::Premultiplied(frame)))
                    .or(Some(None))
            } else {
                None
            }
        }
        #[cfg(windows)]
        (PendingSource::Video(receiver), Some(preview)) => match receiver.try_recv() {
            Ok(Some(snapshot)) => Some(
                ScreenshotImage::from_straight_rgba(snapshot.width, snapshot.height, snapshot.rgba)
                    .map(CapturedPixels::Straight),
            ),
            Err(mpsc::TryRecvError::Empty) if !expired(VIDEO_SNAPSHOT_WAIT) => None,
            _ => Some(
                preview
                    .latest_capture_frame()
                    .map(CapturedPixels::Premultiplied),
            ),
        },
    }
}

/// `"YouTube + Discord"`, or `"YouTube + Discord + 2 more"` for larger sets.
fn combined_title<'a>(titles: impl ExactSizeIterator<Item = &'a str>) -> String {
    let count = titles.len();
    let shown = if count > 3 { 2 } else { count };
    let mut title = titles.take(shown).collect::<Vec<_>>().join(" + ");
    if count > shown {
        title.push_str(&format!(" + {} more", count - shown));
    }
    title
}

/// Decode a still image's file again on a worker thread.
pub(super) fn reread_image(path: PathBuf) -> mpsc::Receiver<Option<ScreenshotImage>> {
    let (sender, receiver) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("pluriview-screenshot-reread".to_owned())
        .spawn(move || {
            let image = crate::media::load(&path).ok().and_then(|asset| {
                let frame = asset.frames.into_iter().next()?;
                ScreenshotImage::from_straight_rgba(frame.width, frame.height, frame.rgba)
            });
            let _ = sender.send(image);
        });
    receiver
}

fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Rect a flying screenshot occupies `t` of the way from the tile to the
/// Capture button: it shrinks while following a gentle upward arc.
fn flight_rect(from: egui::Rect, to: egui::Pos2, t: f32) -> egui::Rect {
    let eased = ease_in_out_cubic(t);
    let landed = from.size() * (LANDED_SIDE / from.width().max(from.height()).max(1.0));
    let size = from.size() + (landed - from.size()) * eased;
    let mut center = from.center() + (to - from.center()) * eased;
    center.y -=
        (from.center().y - to.y).abs().max(80.0) * 0.25 * (std::f32::consts::PI * eased).sin();
    egui::Rect::from_center_size(center, size)
}

impl PluriviewApp {
    pub(super) fn screenshots_dir(&self) -> Option<PathBuf> {
        self.storage.as_ref().map(Storage::screenshots_dir)
    }

    /// Selected and focused tiles keep a shared reference to their newest
    /// uploaded frame, so a screenshot saves exactly what is on screen.
    pub(super) fn sync_screenshot_frame_retention(&mut self) {
        let targets = self.canvas.capture_targets();
        let recording = self.recording_tiles();
        for preview in self.preview_manager.all_mut() {
            preview
                .set_retain_frame(targets.contains(&preview.id) || recording.contains(&preview.id));
        }
    }

    /// Screenshot the focused or selected tiles: several selected tiles make
    /// one image unless `separately` saves each on its own.
    pub(super) fn request_tile_screenshots(&mut self, ctx: &egui::Context, separately: bool) {
        let targets = self.canvas.capture_targets();
        if targets.is_empty() {
            self.canvas_notice = Some((
                "Select or focus a tile to take a screenshot.".to_owned(),
                Instant::now(),
            ));
            return;
        }
        if self.storage.is_none() {
            self.canvas_notice = Some((
                "Screenshots need Pluriview's data folder, which is unavailable.".to_owned(),
                Instant::now(),
            ));
            return;
        }
        let time = LocalTimestamp::now();
        if targets.len() > 1 && !separately {
            self.request_combined_screenshot(targets, time, ctx);
            return;
        }
        for id in targets {
            if self.screenshot_in_progress(id) {
                continue;
            }
            let Some(preview) = self.preview_manager.get(id) else {
                continue;
            };
            if preview.removing.is_some() {
                continue;
            }
            let stem = screenshot::file_stem(&preview.title, time);
            let crop_uv = preview.crop_uv;
            match self.start_tile_screenshot(id, ctx) {
                ShotStart::Ready(pixels) => self.finish_screenshot(id, stem, crop_uv, pixels, ctx),
                ShotStart::Pending(source) => self.screenshots.pending.push(PendingShot {
                    id,
                    stem,
                    crop_uv,
                    requested_at: Instant::now(),
                    source,
                }),
                ShotStart::Unavailable(message) => {
                    self.canvas_notice = Some((message.to_owned(), Instant::now()));
                    continue;
                }
            }
            if let Some(rect) = self.canvas.tile_screen_rect(id, &self.preview_manager) {
                self.screenshots.flashes.push((rect, Instant::now()));
            }
        }
        ctx.request_repaint();
    }

    fn screenshot_in_progress(&self, id: PreviewId) -> bool {
        self.screenshots.pending.iter().any(|shot| shot.id == id)
            || self.screenshots.pending_combined.iter().any(|combined| {
                combined
                    .tiles
                    .iter()
                    .any(|tile| tile.id == id && matches!(tile.frame, CombinedFrame::Pending(_)))
            })
    }

    fn request_combined_screenshot(
        &mut self,
        targets: Vec<PreviewId>,
        time: LocalTimestamp,
        ctx: &egui::Context,
    ) {
        if targets.iter().any(|&id| self.screenshot_in_progress(id)) {
            return;
        }
        let mut order: Vec<(bool, u32, PreviewId, egui::Rect)> = targets
            .into_iter()
            .filter_map(|id| {
                let preview = self.preview_manager.get(id)?;
                if preview.removing.is_some() || preview.is_playlist() {
                    return None;
                }
                let rect = self.canvas.tile_screen_rect(id, &self.preview_manager)?;
                Some((preview.viewport_pin.is_some(), preview.z_order, id, rect))
            })
            .collect();
        // Pinned tiles paint above canvas tiles, then by z-order.
        order.sort_by_key(|&(pinned, z_order, ..)| (pinned, z_order));
        if order.is_empty() {
            self.canvas_notice = Some((
                "None of the selected tiles can be captured.".to_owned(),
                Instant::now(),
            ));
            return;
        }
        let title = combined_title(order.iter().map(|&(_, _, id, _)| {
            self.preview_manager
                .get(id)
                .map_or("Tile", |preview| preview.title.as_str())
        }));
        let mut tiles = Vec::with_capacity(order.len());
        for (_, _, id, rect) in order {
            let crop_uv = self
                .preview_manager
                .get(id)
                .and_then(|preview| preview.crop_uv);
            let frame = match self.start_tile_screenshot(id, ctx) {
                ShotStart::Ready(pixels) => CombinedFrame::Ready(pixels),
                ShotStart::Pending(source) => CombinedFrame::Pending(source),
                ShotStart::Unavailable(_) => CombinedFrame::Missing,
            };
            self.screenshots.flashes.push((rect, Instant::now()));
            tiles.push(CombinedTile {
                id,
                crop_uv,
                rect,
                frame,
            });
        }
        self.screenshots.pending_combined.push(PendingCombined {
            stem: screenshot::file_stem(&title, time),
            requested_at: Instant::now(),
            tiles,
        });
        // Every frame may already be in hand: save without waiting a frame.
        self.combined_screenshot_upkeep(ctx);
        ctx.request_repaint();
    }

    /// Save combined shots whose tiles have all delivered their frames.
    fn combined_screenshot_upkeep(&mut self, ctx: &egui::Context) {
        let mut index = 0;
        while index < self.screenshots.pending_combined.len() {
            let combined = &mut self.screenshots.pending_combined[index];
            let mut waiting = false;
            for tile in &mut combined.tiles {
                let CombinedFrame::Pending(source) = &mut tile.frame else {
                    continue;
                };
                let preview = self.preview_manager.get_mut(tile.id);
                match poll_source(source, preview, combined.requested_at) {
                    None => waiting = true,
                    Some(Some(pixels)) => tile.frame = CombinedFrame::Ready(pixels),
                    Some(None) => tile.frame = CombinedFrame::Missing,
                }
            }
            if waiting {
                index += 1;
                continue;
            }
            let combined = self.screenshots.pending_combined.remove(index);
            let total = combined.tiles.len();
            let parts: Vec<CombinedPart> = combined
                .tiles
                .into_iter()
                .filter_map(|tile| match tile.frame {
                    CombinedFrame::Ready(pixels) => Some(CombinedPart {
                        pixels,
                        crop_uv: tile.crop_uv,
                        rect: tile.rect,
                    }),
                    _ => None,
                })
                .collect();
            if parts.len() < total {
                let message = if parts.is_empty() {
                    "The selected tiles didn't show a frame to capture. Try again."
                } else {
                    "A tile without a frame was left out of the screenshot."
                };
                self.canvas_notice = Some((message.to_owned(), Instant::now()));
            }
            let Some(from) = parts.iter().map(|part| part.rect).reduce(|a, b| a.union(b)) else {
                continue;
            };
            let Some(directory) = self.screenshot_save_dir() else {
                continue;
            };
            let thumbnail = screenshot::combined_thumbnail(&parts, THUMBNAIL_SIDE)
                .and_then(|thumbnail| self.launch_capture_flight_from(from, thumbnail, ctx));
            self.screenshots.saving.push(SavingShot {
                receiver: screenshot::spawn_save_combined(directory, combined.stem, parts),
                thumbnail,
            });
            ctx.request_repaint();
        }
    }

    fn start_tile_screenshot(&mut self, id: PreviewId, ctx: &egui::Context) -> ShotStart {
        let Some(preview) = self.preview_manager.get_mut(id) else {
            return ShotStart::Unavailable("That tile is no longer on the canvas.");
        };
        if preview.is_playlist() {
            return ShotStart::Unavailable("Playlist tiles can't be captured.");
        }
        if preview.is_media() {
            if let Some(image) = preview.current_media_frame().and_then(|frame| {
                ScreenshotImage::from_straight_rgba(frame.width, frame.height, frame.rgba.clone())
            }) {
                return ShotStart::Ready(CapturedPixels::Straight(image));
            }
            return match preview.media_path.clone() {
                Some(path) if preview.media_pixels_released() => {
                    ShotStart::Pending(PendingSource::ImageFile(reread_image(path)))
                }
                _ => ShotStart::Unavailable("This image is still loading."),
            };
        }
        if preview.is_inactive_window() {
            return ShotStart::Unavailable("This window isn't open right now.");
        }

        #[cfg(windows)]
        if preview.is_video() && preview.video_renderer.is_some() && !preview.manually_frozen {
            if let Some(receiver) = self.video_manager.schedule_snapshot(id, ctx) {
                return ShotStart::Pending(PendingSource::Video(receiver));
            }
        }
        #[cfg(not(windows))]
        let _ = ctx;

        let latest = preview.latest_capture_frame();
        let live = preview.is_live_capture() && !preview.manually_frozen;
        if live {
            let already_native = latest.as_ref().is_some_and(|frame| {
                let size = (frame.size[0] as u32, frame.size[1] as u32);
                preview.source_frame_size.is_some_and(|(width, height)| {
                    size == crate::capture::screenshot_capture_size(width, height)
                })
            });
            if !already_native {
                preview.screenshot_wants_native_frame = true;
                return ShotStart::Pending(PendingSource::NativeFrame { fallback: latest });
            }
        }
        match latest {
            Some(frame) => ShotStart::Ready(CapturedPixels::Premultiplied(frame)),
            None if preview.manually_frozen => {
                ShotStart::Unavailable("Resume this tile, then take the screenshot again.")
            }
            None if preview.is_video() => ShotStart::Unavailable("This video is still starting."),
            None if preview.is_browser() => {
                // No frame since the tile was selected: take the next one.
                preview.screenshot_wants_native_frame = true;
                ShotStart::Pending(PendingSource::NativeFrame { fallback: None })
            }
            None => ShotStart::Unavailable("This tile has nothing to capture yet."),
        }
    }

    /// Poll pending frames and saves, after this frame's captures arrived.
    pub(super) fn screenshot_upkeep(&mut self, ctx: &egui::Context) {
        let mut index = 0;
        while index < self.screenshots.pending.len() {
            let shot = &mut self.screenshots.pending[index];
            let preview = self.preview_manager.get_mut(shot.id);
            let Some(pixels) = poll_source(&mut shot.source, preview, shot.requested_at) else {
                index += 1;
                continue;
            };
            let shot = self.screenshots.pending.remove(index);
            match pixels {
                Some(pixels) => {
                    self.finish_screenshot(shot.id, shot.stem, shot.crop_uv, pixels, ctx)
                }
                None => {
                    self.canvas_notice = Some((
                        "The tile didn't show a frame to capture. Try again.".to_owned(),
                        Instant::now(),
                    ));
                }
            }
        }

        self.combined_screenshot_upkeep(ctx);

        let panel = &mut self.screenshots.panel;
        // A hidden panel caches nothing; it rescans when it opens.
        let panel_shown = self.app_config.screenshots_panel_open && !self.canvas_only;
        self.screenshots
            .saving
            .retain(|shot| match shot.receiver.try_recv() {
                Ok(Ok(path)) => {
                    log::info!("Saved a tile screenshot");
                    if panel_shown {
                        panel.add_saved_shot(path, shot.thumbnail.clone());
                    }
                    false
                }
                Ok(Err(error)) => {
                    log::error!("{error}");
                    self.canvas_notice = Some((error, Instant::now()));
                    false
                }
                Err(mpsc::TryRecvError::Empty) => true,
                Err(mpsc::TryRecvError::Disconnected) => false,
            });

        if !self.screenshots.pending.is_empty()
            || !self.screenshots.pending_combined.is_empty()
            || !self.screenshots.saving.is_empty()
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn finish_screenshot(
        &mut self,
        id: PreviewId,
        stem: String,
        crop_uv: Option<(f32, f32, f32, f32)>,
        pixels: CapturedPixels,
        ctx: &egui::Context,
    ) {
        let Some(directory) = self.screenshot_save_dir() else {
            return;
        };
        let saved_thumbnail = pixels
            .thumbnail(crop_uv, THUMBNAIL_SIDE)
            .and_then(|thumbnail| self.launch_capture_flight(id, thumbnail, ctx));
        self.screenshots.saving.push(SavingShot {
            receiver: screenshot::spawn_save(directory, stem, pixels, crop_uv),
            thumbnail: saved_thumbnail,
        });
        ctx.request_repaint();
    }

    /// Fly a thumbnail from the tile into the save folder (or the Capture
    /// button). Returns the texture so the panel can reuse it at once.
    pub(super) fn launch_capture_flight(
        &mut self,
        id: PreviewId,
        thumbnail: egui::ColorImage,
        ctx: &egui::Context,
    ) -> Option<egui::TextureHandle> {
        let from = self.canvas.tile_screen_rect(id, &self.preview_manager)?;
        self.launch_capture_flight_from(from, thumbnail, ctx)
    }

    /// Fly a thumbnail that starts over `from`, in screen points.
    pub(super) fn launch_capture_flight_from(
        &mut self,
        from: egui::Rect,
        thumbnail: egui::ColorImage,
        ctx: &egui::Context,
    ) -> Option<egui::TextureHandle> {
        self.screenshots.next_thumbnail += 1;
        let texture = ctx.load_texture(
            format!("screenshot_flight_{}", self.screenshots.next_thumbnail),
            thumbnail,
            egui::TextureOptions::LINEAR,
        );
        // Fly the visible part of the tile, keeping the image's shape.
        let [width, height] = texture.size().map(|value| value as f32);
        let fit = (from.width() / width).min(from.height() / height);
        let from = egui::Rect::from_center_size(from.center(), egui::vec2(width, height) * fit);
        self.screenshots.flights.push(Flight {
            from,
            texture: texture.clone(),
            started: Instant::now(),
        });
        ctx.request_repaint();
        Some(texture)
    }

    /// Top-bar section for things Pluriview captures itself: one-click
    /// camera and record buttons plus a small menu.
    pub(super) fn capture_section_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (divider, _) = ui.allocate_exact_size(egui::vec2(1.0, 18.0), egui::Sense::hover());
        ui.painter().vline(
            divider.center().x,
            divider.y_range(),
            egui::Stroke::new(1.0, crate::ui_theme::BORDER),
        );
        ui.add_space(6.0);
        let shortcut = self
            .app_config
            .keyboard_shortcuts
            .get(HotkeySlot::ScreenshotTile);
        let camera = ui
            .add(
                egui::Button::new(egui::RichText::new(egui_phosphor::regular::CAMERA).size(16.0))
                    .frame(false)
                    .min_size(egui::vec2(30.0, 26.0)),
            )
            .on_hover_text(format!(
                "Screenshot the selected or focused tile; several selected tiles make one image ({})",
                shortcut.display()
            ));
        self.screenshots.button_rect = Some(camera.rect);
        if camera.clicked() {
            self.request_tile_screenshots(ctx, false);
        }
        self.record_button_ui(ui);
        let panel_shortcut = self
            .app_config
            .keyboard_shortcuts
            .get(HotkeySlot::ToggleScreenshotsPanel);
        let panel_open = self.app_config.screenshots_panel_open;
        let gallery = ui
            .add(
                egui::Button::new(
                    egui::RichText::new(egui_phosphor::regular::IMAGES)
                        .size(16.0)
                        .color(if panel_open {
                            crate::ui_theme::GOLD
                        } else {
                            crate::ui_theme::TEXT
                        }),
                )
                .frame(false)
                .min_size(egui::vec2(30.0, 26.0)),
            )
            .on_hover_text(format!("Screenshots panel ({})", panel_shortcut.display()));
        if gallery.clicked() {
            self.set_screenshots_panel_open(!panel_open);
        }
        ui.menu_button("Capture", |ui| {
            crate::ui_theme::menu(ui);
            crate::ui_theme::section(ui, "SCREENSHOTS");
            if ui
                .add(crate::ui_theme::shortcut_button(
                    format!(
                        "{}   Screenshot Selected Tiles",
                        egui_phosphor::regular::CAMERA
                    ),
                    shortcut,
                ))
                .clicked()
            {
                self.request_tile_screenshots(ctx, false);
                ui.close_menu();
            }
            if self.canvas.capture_targets().len() > 1
                && crate::ui_theme::action(
                    ui,
                    egui_phosphor::regular::SQUARES_FOUR,
                    "Screenshot Each Tile Separately",
                )
                .clicked()
            {
                self.request_tile_screenshots(ctx, true);
                ui.close_menu();
            }
            let mut panel_open = self.app_config.screenshots_panel_open;
            if crate::ui_theme::shortcut_toggle(
                ui,
                &mut panel_open,
                "Screenshots Panel",
                panel_shortcut,
            )
            .clicked()
            {
                self.set_screenshots_panel_open(panel_open);
                ui.close_menu();
            }
            if crate::ui_theme::action(
                ui,
                egui_phosphor::regular::FOLDER_OPEN,
                "Open Screenshots Folder",
            )
            .clicked()
            {
                self.open_screenshots_folder();
                ui.close_menu();
            }
            self.recording_menu_ui(ui);
        });
    }

    pub(super) fn open_screenshots_folder(&mut self) {
        let Some(directory) = self.screenshots_dir() else {
            return;
        };
        if let Err(error) = std::fs::create_dir_all(&directory) {
            self.canvas_notice = Some((
                format!("Could not open the screenshots folder: {error}"),
                Instant::now(),
            ));
            return;
        }
        #[cfg(windows)]
        unsafe {
            let directory = HSTRING::from(directory.as_os_str());
            ShellExecuteW(
                None,
                windows::core::w!("open"),
                &directory,
                None,
                None,
                SW_SHOWNORMAL,
            );
        }
    }

    /// Camera flash on the tile, then a thumbnail that arcs into its folder in
    /// the Screenshots panel (or the camera button), which pulses as it lands.
    pub(super) fn screenshot_animation_ui(&mut self, ctx: &egui::Context) {
        let state = &mut self.screenshots;
        state
            .flashes
            .retain(|(_, started)| started.elapsed().as_secs_f32() < FLASH_SECS);
        if state.flashes.is_empty() && state.flights.is_empty() && state.pulse.is_none() {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("screenshot_animation"),
        ));

        for (rect, started) in &state.flashes {
            let t = started.elapsed().as_secs_f32() / FLASH_SECS;
            let alpha = (1.0 - t).clamp(0.0, 1.0).powi(2) * 0.75;
            painter.rect_filled(*rect, 6.0, egui::Color32::WHITE.gamma_multiply(alpha));
        }

        let screen = ctx.screen_rect();
        // The open panel's save folder first, else the camera button.
        let landing = state.panel.landing_rect.or(state.button_rect);
        let target = landing
            .map(|rect| rect.center())
            .unwrap_or_else(|| egui::pos2(screen.center().x, screen.top() + 12.0));
        let mut landed = false;
        state.flights.retain(|flight| {
            let t = flight.started.elapsed().as_secs_f32() / FLIGHT_SECS;
            if t >= 1.0 {
                landed = true;
                return false;
            }
            let rect = flight_rect(flight.from, target, t);
            let fade = ((1.0 - t) / 0.15).clamp(0.0, 1.0);
            let rounding = 4.0 + 4.0 * ease_in_out_cubic(t);
            painter.rect_filled(
                rect.translate(egui::vec2(0.0, 4.0)).expand(2.0),
                rounding + 2.0,
                egui::Color32::from_black_alpha((90.0 * fade) as u8),
            );
            let mut image = egui::epaint::RectShape::filled(
                rect,
                rounding,
                egui::Color32::WHITE.gamma_multiply(fade),
            );
            image.fill_texture_id = flight.texture.id();
            image.uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
            painter.add(image);
            painter.rect_stroke(
                rect,
                rounding,
                egui::Stroke::new(2.0, egui::Color32::WHITE.gamma_multiply(0.9 * fade)),
            );
            true
        });
        if let (true, Some(rect)) = (landed, landing) {
            state.pulse = Some((Instant::now(), rect));
        }

        if let Some((started, rect)) = state.pulse {
            let t = started.elapsed().as_secs_f32() / PULSE_SECS;
            if t >= 1.0 {
                state.pulse = None;
            } else {
                let alpha = (1.0 - t).powi(2);
                // A ring for the small camera button, a glow for a folder row.
                if rect.width() > 60.0 {
                    painter.rect_stroke(
                        rect.expand(3.0 * t),
                        8.0,
                        egui::Stroke::new(2.0, crate::ui_theme::GOLD.gamma_multiply(alpha)),
                    );
                } else {
                    painter.circle_stroke(
                        rect.center(),
                        10.0 + 10.0 * t,
                        egui::Stroke::new(2.0, crate::ui_theme::GOLD.gamma_multiply(alpha)),
                    );
                }
            }
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flights_start_on_the_tile_and_land_small_on_the_button() {
        let from = egui::Rect::from_min_size(egui::pos2(100.0, 400.0), egui::vec2(320.0, 180.0));
        let target = egui::pos2(900.0, 20.0);
        let start = flight_rect(from, target, 0.0);
        assert!((start.min - from.min).length() < 0.01);
        assert!((start.size() - from.size()).length() < 0.01);
        let end = flight_rect(from, target, 1.0);
        assert!((end.center() - target).length() < 0.01);
        assert!((end.width() - LANDED_SIDE).abs() < 0.01);
        assert!(end.height() < LANDED_SIDE);
    }

    #[test]
    fn combined_titles_name_the_first_tiles() {
        assert_eq!(
            combined_title(["YouTube", "Discord"].into_iter()),
            "YouTube + Discord"
        );
        assert_eq!(combined_title(["A", "B", "C"].into_iter()), "A + B + C");
        assert_eq!(
            combined_title(["A", "B", "C", "D", "E"].into_iter()),
            "A + B + 3 more"
        );
    }

    #[test]
    fn flights_arc_above_the_straight_path() {
        let from = egui::Rect::from_center_size(egui::pos2(0.0, 500.0), egui::vec2(100.0, 100.0));
        let target = egui::pos2(400.0, 100.0);
        let middle = flight_rect(from, target, 0.5);
        assert!(middle.center().y < 300.0);
    }
}
