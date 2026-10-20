use super::*;
use crate::recording::{Clock, Frame, Recorder, MAX_FPS};
use crate::screenshot::{self, CapturedPixels, LocalTimestamp, ScreenshotImage};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

const FRAME_INTERVAL: Duration = Duration::from_millis(1000 / MAX_FPS as u64);
/// How often the flight thumbnail follows the recorded picture.
const THUMBNAIL_REFRESH: Duration = Duration::from_secs(1);
const RECORD_RED: egui::Color32 = egui::Color32::from_rgb(232, 72, 72);

/// Runtime-only recording of one tile.
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

struct ActiveRecording {
    id: PreviewId,
    recorder: Recorder,
    clock: Clock,
    last_sent: Option<Instant>,
    /// Identity of the last frame handed over, so an unchanged source is
    /// not converted again; its picture simply holds in the video.
    last_source: usize,
    /// Small copy of a recent frame that flies into the panel on stop.
    thumbnail: Option<(egui::ColorImage, Instant)>,
    #[cfg(windows)]
    video_readback: Option<mpsc::Receiver<Option<VideoSnapshot>>>,
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

impl PluriviewApp {
    pub(super) fn recording_tile(&self) -> Option<PreviewId> {
        self.recording.active.as_ref().map(|active| active.id)
    }

    /// Start recording the selected or focused tile, or stop the recording.
    pub(super) fn toggle_recording(&mut self, ctx: &egui::Context) {
        if self.recording.active.is_some() {
            self.stop_recording(ctx);
            return;
        }
        let notice = |app: &mut Self, message: &str| {
            app.canvas_notice = Some((message.to_owned(), Instant::now()));
        };
        let targets = self.canvas.capture_targets();
        let [id] = targets[..] else {
            let message = if targets.is_empty() {
                "Select or focus a tile to record it."
            } else {
                "Select just one tile to record."
            };
            return notice(self, message);
        };
        // Recordings join the screenshots in the folder new shots go to.
        let Some(directory) = self.screenshot_save_dir() else {
            return notice(
                self,
                "Recording needs Pluriview's data folder, which is unavailable.",
            );
        };
        let Some(preview) = self.preview_manager.get_mut(id) else {
            return;
        };
        if preview.is_playlist() {
            return notice(self, "Playlist tiles can't be recorded.");
        }
        if preview.is_inactive_window() {
            return notice(self, "This window isn't open right now.");
        }
        if preview.is_media() && preview.media_pixels_released() {
            return notice(
                self,
                "Still images don't change; take a screenshot instead.",
            );
        }
        if let Err(error) = std::fs::create_dir_all(&directory) {
            return notice(
                self,
                &format!("Could not create the screenshots folder: {error}"),
            );
        }
        // Windows pick up their own process tree's sound. Browser and video
        // tiles play inside Pluriview, so they record Pluriview's sound.
        let audio_pid = if preview.is_window_capture() {
            preview
                .window_handle
                .as_ref()
                .map(|handle| handle.process_id)
        } else if preview.is_browser() || preview.is_video() {
            Some(std::process::id())
        } else {
            None
        };
        let stem = screenshot::file_stem(&preview.title, LocalTimestamp::now());
        let path = unique_path(&directory, &stem);
        preview.recording = true;
        let started = Instant::now();
        self.recording.active = Some(ActiveRecording {
            id,
            recorder: Recorder::start(path, audio_pid.filter(|pid| *pid != 0), started),
            clock: Clock::new(started),
            last_sent: None,
            last_source: 0,
            thumbnail: None,
            #[cfg(windows)]
            video_readback: None,
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

    /// Finish the file and fly the recording's picture into its folder in
    /// the Screenshots panel (or the Capture button), like a screenshot.
    pub(super) fn stop_recording(&mut self, ctx: &egui::Context) {
        let Some(active) = self.recording.active.take() else {
            return;
        };
        if let Some(preview) = self.preview_manager.get_mut(active.id) {
            preview.recording = false;
        }
        let length = active.clock.media_time(Instant::now());
        let thumbnail = active
            .thumbnail
            .and_then(|(image, _)| self.launch_capture_flight(active.id, image, ctx));
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

    /// Hand the recorded tile's newest frame to the encoder, at most
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
            let id = active.id;
            self.recording.active = None;
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.recording = false;
            }
            self.canvas_notice = Some((error, Instant::now()));
            return;
        }
        let id = active.id;
        let Some(preview) = self.preview_manager.get_mut(id) else {
            // The tile was removed: keep what was recorded.
            return self.stop_recording(ctx);
        };
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

        let crop_uv = preview.crop_uv;
        let mut pixels = None;
        #[cfg(windows)]
        if preview.is_video() && preview.video_renderer.is_some() {
            match active.video_readback.as_ref().map(mpsc::Receiver::try_recv) {
                Some(Ok(snapshot)) => {
                    active.video_readback = None;
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
                    active.video_readback = self.video_manager.schedule_snapshot(id, ctx);
                }
            }
        }
        if preview.is_media() {
            if let Some(frame) = preview.current_media_frame() {
                let source = frame.rgba.as_ptr() as usize;
                if source != active.last_source {
                    active.last_source = source;
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
            if source != active.last_source {
                active.last_source = source;
                pixels = Some(CapturedPixels::Premultiplied(frame));
            }
        }
        let Some(pixels) = pixels else {
            return;
        };
        if active
            .thumbnail
            .as_ref()
            .is_none_or(|(_, made)| made.elapsed() >= THUMBNAIL_REFRESH)
        {
            if let Some(image) = pixels.thumbnail(crop_uv, super::screenshots::THUMBNAIL_SIDE) {
                active.thumbnail = Some((image, now));
            }
        }
        let frame = Frame {
            pixels,
            crop_uv,
            at: now,
        };
        if active.recorder.push(frame) {
            active.last_sent = Some(now);
        } else {
            // Encoder busy: try again with a newer frame next time.
            active.last_source = 0;
        }
    }

    /// While recording: a pulsing red outline around the tile and a badge
    /// with the elapsed time (gold and still while paused). If the tile is
    /// off screen, the badge sits at the top of the canvas instead.
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
        let tile = self
            .canvas
            .tile_screen_rect(active.id, &self.preview_manager)
            .filter(|rect| rect.intersects(canvas));
        if let Some(rect) = tile {
            // Drawn just outside the tile so it also frames a browser tile
            // while its live page window sits on top.
            let strength = if paused {
                0.9
            } else {
                0.55 + 0.45 * (time * std::f64::consts::PI).sin().abs() as f32
            };
            painter.rect_stroke(
                rect.expand(3.0),
                8.0,
                egui::Stroke::new(3.0, color.gamma_multiply(strength)),
            );
        }

        let elapsed = format_elapsed(active.clock.media_time(Instant::now()));
        let mut label = if paused {
            format!("PAUSED  {elapsed}")
        } else {
            format!("REC  {elapsed}")
        };
        if tile.is_none() {
            if let Some(preview) = self.preview_manager.get(active.id) {
                label.push_str(&format!("  ·  {}", short_title(&preview.title)));
            }
        }
        let galley =
            painter.layout_no_wrap(label, egui::FontId::monospace(13.0), egui::Color32::WHITE);
        let size = galley.size() + egui::vec2(34.0, 10.0);
        let corner = match tile {
            // Above the tile's top-left corner, or just inside it when the
            // tile touches the top of the canvas.
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
                    "Record the selected or focused tile ({})",
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
            format!("{}   Record Selected Tile", egui_phosphor::regular::RECORD)
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
    use super::format_elapsed;
    use std::time::Duration;

    #[test]
    fn elapsed_time_reads_like_a_clock() {
        assert_eq!(format_elapsed(Duration::from_millis(9_999)), "0:09");
        assert_eq!(format_elapsed(Duration::from_secs(754)), "12:34");
        assert_eq!(format_elapsed(Duration::from_secs(3_725)), "1:02:05");
    }
}
