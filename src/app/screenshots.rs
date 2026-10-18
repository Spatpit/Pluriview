use super::*;
use crate::screenshot::{self, CapturedPixels, LocalTimestamp, ScreenshotImage};
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
const THUMBNAIL_SIDE: u32 = 240;
const LANDED_SIDE: f32 = 26.0;

/// Runtime-only screenshot work and its animations.
#[derive(Default)]
pub(super) struct ScreenshotState {
    pending: Vec<PendingShot>,
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

/// Decode a still image's file again on a worker thread.
fn reread_image(path: PathBuf) -> mpsc::Receiver<Option<ScreenshotImage>> {
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
        for preview in self.preview_manager.all_mut() {
            preview.set_retain_frame(targets.contains(&preview.id));
        }
    }

    pub(super) fn request_tile_screenshots(&mut self, ctx: &egui::Context) {
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
        for id in targets {
            if self.screenshots.pending.iter().any(|shot| shot.id == id) {
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
            let expired = |wait: Duration| shot.requested_at.elapsed() >= wait;
            let preview = self.preview_manager.get_mut(shot.id);
            let outcome: Option<Option<CapturedPixels>> = match (&mut shot.source, preview) {
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
                        ScreenshotImage::from_straight_rgba(
                            snapshot.width,
                            snapshot.height,
                            snapshot.rgba,
                        )
                        .map(CapturedPixels::Straight),
                    ),
                    Err(mpsc::TryRecvError::Empty) if !expired(VIDEO_SNAPSHOT_WAIT) => None,
                    _ => Some(
                        preview
                            .latest_capture_frame()
                            .map(CapturedPixels::Premultiplied),
                    ),
                },
            };
            let Some(pixels) = outcome else {
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

        if !self.screenshots.pending.is_empty() || !self.screenshots.saving.is_empty() {
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
        let thumbnail = pixels.thumbnail(crop_uv, THUMBNAIL_SIDE);
        let mut saved_thumbnail = None;
        let from = self.canvas.tile_screen_rect(id, &self.preview_manager);
        if let (Some(thumbnail), Some(from)) = (thumbnail, from) {
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
            saved_thumbnail = Some(texture.clone());
            self.screenshots.flights.push(Flight {
                from,
                texture,
                started: Instant::now(),
            });
        }
        self.screenshots.saving.push(SavingShot {
            receiver: screenshot::spawn_save(directory, stem, pixels, crop_uv),
            thumbnail: saved_thumbnail,
        });
        ctx.request_repaint();
    }

    /// Top-bar section for things Pluriview captures itself. A one-click
    /// camera plus a small menu; recording can join it later.
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
                "Screenshot the selected or focused tile ({})",
                shortcut.display()
            ));
        self.screenshots.button_rect = Some(camera.rect);
        if camera.clicked() {
            self.request_tile_screenshots(ctx);
        }
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
                        "{}   Screenshot Selected Tile",
                        egui_phosphor::regular::CAMERA
                    ),
                    shortcut,
                ))
                .clicked()
            {
                self.request_tile_screenshots(ctx);
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
    fn flights_arc_above_the_straight_path() {
        let from = egui::Rect::from_center_size(egui::pos2(0.0, 500.0), egui::vec2(100.0, 100.0));
        let target = egui::pos2(400.0, 100.0);
        let middle = flight_rect(from, target, 0.5);
        assert!(middle.center().y < 300.0);
    }
}
