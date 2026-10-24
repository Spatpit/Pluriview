use super::*;
use crate::recording::{Clock, Frame, Layer, Recorder, MAX_COMBINED_SIZE, MAX_FPS};
use crate::screenshot::{self, CapturedPixels, CombinedPart, LocalTimestamp, ScreenshotImage};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

const FRAME_INTERVAL: Duration = Duration::from_millis(1000 / MAX_FPS as u64);
/// How often the flight thumbnail follows the recorded picture.
const THUMBNAIL_REFRESH: Duration = Duration::from_secs(1);
const RECORD_RED: egui::Color32 = egui::Color32::from_rgb(232, 72, 72);

/// Runtime-only recording of one tile, or several laid out as on the canvas.
#[derive(Default)]
pub(super) struct RecordingState {
    active: Option<ActiveRecording>,
    /// Recordings whose worker is still finishing the MP4.
    finishing: Vec<FinishingRecording>,
}

struct FinishingRecording {
    receiver: mpsc::Receiver<Result<PathBuf, String>>,
    /// The flight thumbnail, reused by the Screenshots panel.
    thumbnail: Option<egui::TextureHandle>,
    length: Duration,
}

struct RecordedTile {
    id: PreviewId,
    /// Newest picture, kept so every video frame shows every tile even when
    /// only one of them changed.
    latest: Option<Arc<CapturedPixels>>,
    /// Identity of `latest`'s source, so an unchanged tile is not copied
    /// again.
    last_source: usize,
    /// A still image whose decoded pixels were released, read again once.
    reread: Option<mpsc::Receiver<Option<ScreenshotImage>>>,
    #[cfg(windows)]
    video_readback: Option<mpsc::Receiver<Option<VideoSnapshot>>>,
}

struct ActiveRecording {
    /// Bottom to top, as the canvas paints them.
    tiles: Vec<RecordedTile>,
    /// Canvas area shown by a recording of several tiles, fixed when it
    /// starts; `None` records a single tile filling the video.
    area: Option<egui::Rect>,
    recorder: Recorder,
    clock: Clock,
    last_sent: Option<Instant>,
    /// Where each tile sat in the last frame sent, to notice moves.
    last_places: Vec<Option<egui::Rect>>,
    /// Small copy of a recent frame that flies into the panel on stop.
    thumbnail: Option<(egui::ColorImage, Instant)>,
}

/// Elapsed recording time as `m:ss`, or `h:mm:ss` past an hour.
fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// `rect` as fractions of `area`.
fn place_in(area: egui::Rect, rect: egui::Rect) -> egui::Rect {
    let size = area.size().max(egui::vec2(1.0, 1.0));
    egui::Rect::from_min_max(
        ((rect.min - area.min) / size).to_pos2(),
        ((rect.max - area.min) / size).to_pos2(),
    )
}

impl PluriviewApp {
    pub(super) fn recording_tiles(&self) -> Vec<PreviewId> {
        self.recording
            .active
            .as_ref()
            .map_or_else(Vec::new, |active| {
                active.tiles.iter().map(|tile| tile.id).collect()
            })
    }

    /// A tile's rect in canvas coordinates as currently drawn, so panning
    /// or zooming the canvas does not move it within a combined recording.
    fn recorded_canvas_rect(&self, id: PreviewId) -> Option<egui::Rect> {
        let canvas = self.canvas.last_screen_rect?;
        let screen = self.canvas.tile_screen_rect(id, &self.preview_manager)?;
        Some(egui::Rect::from_two_pos(
            self.canvas.screen_to_canvas(screen.min, canvas),
            self.canvas.screen_to_canvas(screen.max, canvas),
        ))
    }

    /// Start recording the focused or selected tiles, or stop recording.
    /// Several selected tiles record into one video laid out like the canvas.
    pub(super) fn toggle_recording(&mut self, ctx: &egui::Context) {
        if self.recording.active.is_some() {
            self.stop_recording(ctx);
            return;
        }
        let notice = |app: &mut Self, message: &str| {
            app.canvas_notice = Some((message.to_owned(), Instant::now()));
        };
        let targets = self.canvas.capture_targets();
        if targets.is_empty() {
            return notice(self, "Select or focus tiles to record them.");
        }
        let mut order: Vec<(bool, u32, PreviewId)> = targets
            .iter()
            .filter_map(|&id| {
                let preview = self.preview_manager.get(id)?;
                (preview.removing.is_none()
                    && !preview.is_playlist()
                    && !preview.is_inactive_window())
                .then_some((preview.viewport_pin.is_some(), preview.z_order, id))
            })
            .collect();
        if order.is_empty() {
            let message = if targets.len() == 1 {
                "This tile can't be recorded right now."
            } else {
                "None of the selected tiles can be recorded right now."
            };
            return notice(self, message);
        }
        // Pinned tiles paint above canvas tiles, then by z-order.
        order.sort_by_key(|&(pinned, z_order, _)| (pinned, z_order));
        let ids: Vec<PreviewId> = order.into_iter().map(|(_, _, id)| id).collect();

        // Several tiles: the video shows the area around them, fitted
        // inside 1080p at no more than twice their on-screen pixels.
        let (area, size) = if ids.len() > 1 {
            let rects: Vec<egui::Rect> = ids
                .iter()
                .filter_map(|&id| self.recorded_canvas_rect(id))
                .collect();
            let screen = ids
                .iter()
                .filter_map(|&id| self.canvas.tile_screen_rect(id, &self.preview_manager))
                .reduce(|a, b| a.union(b));
            let (Some(area), Some(screen)) = (rects.into_iter().reduce(|a, b| a.union(b)), screen)
            else {
                return notice(self, "The selected tiles aren't on the canvas yet.");
            };
            let pixels = screen.size() * ctx.pixels_per_point() * 2.0;
            let size = crate::recording::fitted_size(pixels.x, pixels.y, MAX_COMBINED_SIZE);
            (Some(area), Some(size))
        } else {
            (None, None)
        };

        // Recordings join the screenshots in the folder new shots go to.
        let Some(directory) = self.screenshot_save_dir() else {
            return notice(
                self,
                "Recording needs Pluriview's data folder, which is unavailable.",
            );
        };
        if let Err(error) = std::fs::create_dir_all(&directory) {
            return notice(
                self,
                &format!("Could not create the screenshots folder: {error}"),
            );
        }

        let single = ids.len() == 1;
        let mut audio_pids = Vec::new();
        let mut titles = Vec::new();
        let mut tiles = Vec::new();
        for &id in &ids {
            let Some(preview) = self.preview_manager.get_mut(id) else {
                continue;
            };
            // Windows pick up their own process tree's sound. Browser and
            // video tiles play inside Pluriview, so they bring its sound.
            let pid = if preview.is_window_capture() {
                preview
                    .window_handle
                    .as_ref()
                    .map(|handle| handle.process_id)
            } else if preview.is_browser() || preview.is_video() {
                Some(std::process::id())
            } else {
                None
            };
            if let Some(pid) = pid.filter(|pid| *pid != 0 && !audio_pids.contains(pid)) {
                audio_pids.push(pid);
            }
            let reread = (preview.is_media() && preview.media_pixels_released())
                .then(|| preview.media_path.clone())
                .flatten()
                .map(super::screenshots::reread_image);
            preview.recording = true;
            preview.recording_full_size = single;
            titles.push(preview.title.clone());
            tiles.push(RecordedTile {
                id,
                latest: None,
                last_source: 0,
                reread,
                #[cfg(windows)]
                video_readback: None,
            });
        }
        let title = match titles.as_slice() {
            [only] => only.clone(),
            [first, second] => format!("{first} + {second}"),
            [first, second, rest @ ..] => format!("{first} + {second} + {} more", rest.len()),
            [] => "Tiles".to_owned(),
        };
        let path = unique_path(
            &directory,
            &screenshot::file_stem(&title, LocalTimestamp::now()),
        );
        let started = Instant::now();
        self.recording.active = Some(ActiveRecording {
            last_places: vec![None; tiles.len()],
            tiles,
            area,
            recorder: Recorder::start(path, audio_pids, size, started),
            clock: Clock::new(started),
            last_sent: None,
            thumbnail: None,
        });
        ctx.request_repaint();
    }

    pub(super) fn toggle_recording_pause(&mut self) {
        let Some(active) = self.recording.active.as_mut() else {
            return;
        };
        let now = Instant::now();
        if active.clock.paused() {
            active.clock.resume(now);
            active.recorder.resume(now);
        } else {
            active.clock.pause(now);
            active.recorder.pause(now);
        }
    }

    fn release_recorded_tiles(&mut self, ids: &[PreviewId]) {
        for &id in ids {
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.recording = false;
                preview.recording_full_size = false;
            }
        }
    }

    /// Union of the recorded tiles' on-screen rects.
    fn recorded_screen_rect(&self, ids: &[PreviewId]) -> Option<egui::Rect> {
        ids.iter()
            .filter_map(|&id| self.canvas.tile_screen_rect(id, &self.preview_manager))
            .reduce(|a, b| a.union(b))
    }

    /// Finish the file and fly the recording's picture into its folder in
    /// the Screenshots panel (or the Capture button), like a screenshot.
    pub(super) fn stop_recording(&mut self, ctx: &egui::Context) {
        let Some(active) = self.recording.active.take() else {
            return;
        };
        let ids: Vec<PreviewId> = active.tiles.iter().map(|tile| tile.id).collect();
        let from = self.recorded_screen_rect(&ids);
        self.release_recorded_tiles(&ids);
        let length = active.clock.media_time(Instant::now());
        let thumbnail = match (active.thumbnail, from) {
            (Some((image, _)), Some(from)) => self.launch_capture_flight_from(from, image, ctx),
            _ => None,
        };
        self.recording.finishing.push(FinishingRecording {
            receiver: active.recorder.stop(),
            thumbnail,
            length,
        });
        self.canvas_notice = Some((
            format!("Saving the {} recording…", format_elapsed(length)),
            Instant::now(),
        ));
    }

    /// Hand the recorded tiles' newest frames to the encoder, at most
    /// `MAX_FPS` times a second. Runs after this frame's captures arrived.
    pub(super) fn recording_upkeep(&mut self, ctx: &egui::Context) {
        let panel_shown = self.app_config.screenshots_panel_open && !self.canvas_only;
        let panel = &mut self.screenshots.panel;
        let mut notice = None;
        self.recording
            .finishing
            .retain(|finishing| match finishing.receiver.try_recv() {
                Ok(Ok(path)) => {
                    if panel_shown {
                        panel.add_saved_shot(path, finishing.thumbnail.clone());
                    }
                    notice = Some(format!(
                        "Saved the {} recording with your screenshots.",
                        format_elapsed(finishing.length)
                    ));
                    false
                }
                Ok(Err(error)) => {
                    log::error!("{error}");
                    notice = Some(error);
                    false
                }
                Err(mpsc::TryRecvError::Empty) => true,
                Err(mpsc::TryRecvError::Disconnected) => false,
            });
        if let Some(message) = notice {
            self.canvas_notice = Some((message, Instant::now()));
        }
        if !self.recording.finishing.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        let Some(active) = self.recording.active.as_mut() else {
            return;
        };
        if let Some(error) = active.recorder.failure() {
            let ids: Vec<PreviewId> = active.tiles.iter().map(|tile| tile.id).collect();
            self.recording.active = None;
            self.release_recorded_tiles(&ids);
            self.canvas_notice = Some((error, Instant::now()));
            return;
        }
        // Removed tiles leave the recording; it ends with the last one.
        active
            .tiles
            .retain(|tile| self.preview_manager.get(tile.id).is_some());
        if active.tiles.is_empty() {
            return self.stop_recording(ctx);
        }
        // Taken out while frames are gathered, which reads the canvas.
        let Some(mut active) = self.recording.active.take() else {
            return;
        };
        self.feed_recording(&mut active, ctx);
        self.recording.active = Some(active);
    }

    fn feed_recording(&mut self, active: &mut ActiveRecording, ctx: &egui::Context) {
        // Keep the timer and the REC badge moving even while nothing else
        // repaints.
        ctx.request_repaint_after(if active.clock.paused() {
            Duration::from_millis(250)
        } else {
            FRAME_INTERVAL
        });
        let now = Instant::now();
        if active.clock.paused()
            || active
                .last_sent
                .is_some_and(|sent| now.duration_since(sent) < FRAME_INTERVAL)
        {
            return;
        }

        let mut changed = false;
        for tile in &mut active.tiles {
            let Some(preview) = self.preview_manager.get_mut(tile.id) else {
                continue;
            };
            let mut pixels = None;
            if let Some(receiver) = &tile.reread {
                match receiver.try_recv() {
                    Ok(image) => {
                        tile.reread = None;
                        pixels = image.map(CapturedPixels::Straight);
                    }
                    Err(mpsc::TryRecvError::Disconnected) => tile.reread = None,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            #[cfg(windows)]
            if preview.is_video() && preview.video_renderer.is_some() {
                match tile.video_readback.as_ref().map(mpsc::Receiver::try_recv) {
                    Some(Ok(snapshot)) => {
                        tile.video_readback = None;
                        pixels = snapshot.and_then(|snapshot| {
                            ScreenshotImage::from_straight_rgba(
                                snapshot.width,
                                snapshot.height,
                                snapshot.rgba,
                            )
                            .map(CapturedPixels::Straight)
                        });
                    }
                    Some(Err(mpsc::TryRecvError::Empty)) => {}
                    None | Some(Err(mpsc::TryRecvError::Disconnected)) => {
                        tile.video_readback = self.video_manager.schedule_snapshot(tile.id, ctx);
                    }
                }
            }
            if preview.is_media() {
                if let Some(frame) = preview.current_media_frame() {
                    let source = frame.rgba.as_ptr() as usize;
                    if source != tile.last_source {
                        tile.last_source = source;
                        pixels = ScreenshotImage::from_straight_rgba(
                            frame.width,
                            frame.height,
                            frame.rgba.clone(),
                        )
                        .map(CapturedPixels::Straight);
                    }
                }
            } else if let Some(frame) = preview.latest_capture_frame() {
                let source = Arc::as_ptr(&frame) as usize;
                if source != tile.last_source {
                    tile.last_source = source;
                    pixels = Some(CapturedPixels::Premultiplied(frame));
                }
            }
            if let Some(pixels) = pixels {
                tile.latest = Some(Arc::new(pixels));
                changed = true;
            }
        }

        let places: Vec<Option<egui::Rect>> = match active.area {
            None => vec![None; active.tiles.len()],
            Some(area) => active
                .tiles
                .iter()
                .map(|tile| {
                    self.recorded_canvas_rect(tile.id)
                        .map(|rect| place_in(area, rect))
                })
                .collect(),
        };
        let moved = places != active.last_places;
        if !changed && !moved && active.last_sent.is_some() {
            return;
        }
        let layers: Vec<Layer> = active
            .tiles
            .iter()
            .zip(&places)
            .filter_map(|(tile, place)| {
                let pixels = tile.latest.clone()?;
                // A tile of a combined recording that is not on the canvas
                // right now (still loading) is left out of this frame.
                if active.area.is_some() && place.is_none() {
                    return None;
                }
                let crop_uv = self.preview_manager.get(tile.id)?.crop_uv;
                Some(Layer {
                    pixels,
                    crop_uv,
                    place: *place,
                })
            })
            .collect();
        if layers.is_empty() {
            return;
        }
        if active
            .thumbnail
            .as_ref()
            .is_none_or(|(_, made)| made.elapsed() >= THUMBNAIL_REFRESH)
        {
            if let Some(image) = recording_thumbnail(&layers) {
                active.thumbnail = Some((image, now));
            }
        }
        let frame = Frame { layers, at: now };
        if active.recorder.push(frame) {
            active.last_sent = Some(now);
            active.last_places = places;
        } else {
            // Encoder busy: send again next time even if nothing changes.
            active.last_places.clear();
        }
    }

    /// While recording: a pulsing red outline around each recorded tile and
    /// a badge with the elapsed time (gold and still while paused). If the
    /// tiles are off screen, the badge sits at the top of the canvas.
    pub(super) fn recording_overlay_ui(&self, ctx: &egui::Context) {
        let Some(active) = &self.recording.active else {
            return;
        };
        let Some(canvas) = self.canvas.last_screen_rect else {
            return;
        };
        let paused = active.clock.paused();
        let color = if paused {
            crate::ui_theme::GOLD
        } else {
            RECORD_RED
        };
        let time = ctx.input(|input| input.time);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("recording_overlay"),
        ));
        let strength = if paused {
            0.9
        } else {
            0.55 + 0.45 * (time * std::f64::consts::PI).sin().abs() as f32
        };
        let visible: Vec<egui::Rect> = active
            .tiles
            .iter()
            .filter_map(|tile| self.canvas.tile_screen_rect(tile.id, &self.preview_manager))
            .filter(|rect| rect.intersects(canvas))
            .collect();
        for rect in &visible {
            // Drawn just outside the tile so it also frames a browser tile
            // while its live page window sits on top.
            painter.rect_stroke(
                rect.expand(3.0),
                8.0,
                egui::Stroke::new(3.0, color.gamma_multiply(strength)),
            );
        }
        let shown = visible.into_iter().reduce(|a, b| a.union(b));

        let elapsed = format_elapsed(active.clock.media_time(Instant::now()));
        let mut label = if paused {
            format!("PAUSED  {elapsed}")
        } else {
            format!("REC  {elapsed}")
        };
        if active.tiles.len() > 1 {
            label.push_str(&format!("  ·  {} tiles", active.tiles.len()));
        } else if shown.is_none() {
            if let Some(preview) = self.preview_manager.get(active.tiles[0].id) {
                label.push_str(&format!("  ·  {}", short_title(&preview.title)));
            }
        }
        let galley =
            painter.layout_no_wrap(label, egui::FontId::monospace(13.0), egui::Color32::WHITE);
        let size = galley.size() + egui::vec2(34.0, 10.0);
        let corner = match shown {
            // Above the tiles' top-left corner, or just inside it when they
            // touch the top of the canvas.
            Some(rect) if rect.top() - size.y - 8.0 >= canvas.top() => {
                rect.left_top() + egui::vec2(0.0, -size.y - 8.0)
            }
            Some(rect) => rect.left_top() + egui::vec2(8.0, 8.0),
            None => egui::pos2(canvas.center().x - size.x / 2.0, canvas.top() + 12.0),
        };
        let x = corner.x.clamp(
            canvas.left() + 4.0,
            (canvas.right() - size.x - 4.0).max(canvas.left()),
        );
        let badge = egui::Rect::from_min_size(egui::pos2(x, corner.y), size);
        painter.rect_filled(badge, size.y / 2.0, egui::Color32::from_black_alpha(215));
        painter.rect_stroke(badge, size.y / 2.0, egui::Stroke::new(1.5, color));
        let mark = egui::pos2(badge.left() + 15.0, badge.center().y);
        if paused {
            for offset in [-3.0, 3.0] {
                painter.rect_filled(
                    egui::Rect::from_center_size(
                        mark + egui::vec2(offset, 0.0),
                        egui::vec2(3.0, 10.0),
                    ),
                    1.0,
                    color,
                );
            }
        } else {
            // A blinking dot, like a camera's tally light.
            let on = (time * 2.0) as i64 % 2 == 0;
            painter.circle_filled(
                mark,
                5.0,
                if on {
                    color
                } else {
                    color.gamma_multiply(0.35)
                },
            );
        }
        painter.galley(
            egui::pos2(
                badge.left() + 26.0,
                badge.center().y - galley.size().y / 2.0,
            ),
            galley,
            egui::Color32::WHITE,
        );
    }

    /// Record button beside the camera; while recording it becomes a red
    /// dot (gold when paused) with the elapsed time and stops on click.
    pub(super) fn record_button_ui(&mut self, ui: &mut egui::Ui) {
        let shortcut = self
            .app_config
            .keyboard_shortcuts
            .get(HotkeySlot::RecordTile);
        let button = match &self.recording.active {
            None => ui
                .add(
                    egui::Button::new(
                        egui::RichText::new(egui_phosphor::regular::RECORD).size(16.0),
                    )
                    .frame(false)
                    .min_size(egui::vec2(30.0, 26.0)),
                )
                .on_hover_text(format!(
                    "Record the focused or selected tiles ({})",
                    shortcut.display()
                )),
            Some(active) => {
                let paused = active.clock.paused();
                let (icon, color) = if paused {
                    (egui_phosphor::regular::PAUSE, crate::ui_theme::GOLD)
                } else {
                    (egui_phosphor::regular::RECORD, RECORD_RED)
                };
                let elapsed = format_elapsed(active.clock.media_time(Instant::now()));
                let pause_shortcut = self
                    .app_config
                    .keyboard_shortcuts
                    .get(HotkeySlot::PauseRecording);
                ui.add(
                    egui::Button::new(
                        egui::RichText::new(format!("{icon} {elapsed}"))
                            .size(13.5)
                            .monospace()
                            .color(color),
                    )
                    .frame(false)
                    .min_size(egui::vec2(30.0, 26.0)),
                )
                .on_hover_text(format!(
                    "{} — click or press {} to stop and save, {} to {}",
                    if paused {
                        "Recording paused"
                    } else {
                        "Recording"
                    },
                    shortcut.display(),
                    pause_shortcut.display(),
                    if paused { "resume" } else { "pause" },
                ))
            }
        };
        if button.clicked() {
            self.toggle_recording(ui.ctx());
        }
    }

    /// Recording entries in the Capture menu.
    pub(super) fn recording_menu_ui(&mut self, ui: &mut egui::Ui) {
        crate::ui_theme::section(ui, "RECORDING");
        let shortcuts = &self.app_config.keyboard_shortcuts;
        let (record, pause) = (
            shortcuts.get(HotkeySlot::RecordTile),
            shortcuts.get(HotkeySlot::PauseRecording),
        );
        let recording = self
            .recording
            .active
            .as_ref()
            .map(|active| active.clock.paused());
        let title = if recording.is_some() {
            format!("{}   Stop and Save Recording", egui_phosphor::regular::STOP)
        } else {
            format!("{}   Record Selected Tiles", egui_phosphor::regular::RECORD)
        };
        if ui
            .add(crate::ui_theme::shortcut_button(title, record))
            .clicked()
        {
            self.toggle_recording(ui.ctx());
            ui.close_menu();
        }
        let (icon, title) = if recording == Some(true) {
            (egui_phosphor::regular::PLAY, "Resume Recording")
        } else {
            (egui_phosphor::regular::PAUSE, "Pause Recording")
        };
        if ui
            .add_enabled(
                recording.is_some(),
                crate::ui_theme::shortcut_button(format!("{icon}   {title}"), pause),
            )
            .clicked()
        {
            self.toggle_recording_pause();
            ui.close_menu();
        }
    }
}

/// Small picture of the frame for the stop animation: one tile's own
/// thumbnail, or the tiles laid out together.
fn recording_thumbnail(layers: &[Layer]) -> Option<egui::ColorImage> {
    let side = super::screenshots::THUMBNAIL_SIDE;
    if let [layer] = layers {
        if layer.place.is_none() {
            return layer.pixels.thumbnail(layer.crop_uv, side);
        }
    }
    let parts: Vec<CombinedPart> = layers
        .iter()
        .filter_map(|layer| {
            Some(CombinedPart {
                pixels: CapturedPixels::Premultiplied(Arc::new(
                    layer.pixels.thumbnail(layer.crop_uv, side)?,
                )),
                crop_uv: None,
                // Fractions of the video, scaled up to a workable size.
                rect: egui::Rect::from_min_max(
                    (layer.place?.min.to_vec2() * 1000.0).to_pos2(),
                    (layer.place?.max.to_vec2() * 1000.0).to_pos2(),
                ),
            })
        })
        .collect();
    screenshot::combined_thumbnail(&parts, side)
}

/// Keep the off-screen badge to one short line.
fn short_title(title: &str) -> String {
    const MAX_CHARS: usize = 32;
    let mut short: String = title.chars().take(MAX_CHARS).collect();
    if title.chars().count() > MAX_CHARS {
        short.push('…');
    }
    short
}

/// `stem.mp4`, or `stem (2).mp4` and so on when a recording of that name
/// already exists from the same second.
fn unique_path(directory: &std::path::Path, stem: &str) -> PathBuf {
    let mut path = directory.join(format!("{stem}.mp4"));
    let mut number = 2;
    while path.exists() {
        path = directory.join(format!("{stem} ({number}).mp4"));
        number += 1;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::{format_elapsed, place_in};
    use eframe::egui;
    use std::time::Duration;

    #[test]
    fn elapsed_time_reads_like_a_clock() {
        assert_eq!(format_elapsed(Duration::from_millis(9_999)), "0:09");
        assert_eq!(format_elapsed(Duration::from_secs(754)), "12:34");
        assert_eq!(format_elapsed(Duration::from_secs(3_725)), "1:02:05");
    }

    #[test]
    fn tiles_are_placed_as_fractions_of_the_recorded_area() {
        let area = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(400.0, 200.0));
        let tile = egui::Rect::from_min_size(egui::pos2(300.0, 50.0), egui::vec2(200.0, 100.0));
        let place = place_in(area, tile);
        assert_eq!(place.min, egui::pos2(0.5, 0.0));
        assert_eq!(place.max, egui::pos2(1.0, 0.5));
    }
}
