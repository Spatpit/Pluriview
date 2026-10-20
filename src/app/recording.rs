use super::*;
use crate::recording::{Clock, Frame, Recorder, MAX_FPS};
use crate::screenshot::{self, CapturedPixels, LocalTimestamp, ScreenshotImage};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

const FRAME_INTERVAL: Duration = Duration::from_millis(1000 / MAX_FPS as u64);

/// Runtime-only recording of one tile.
#[derive(Default)]
pub(super) struct RecordingState {
    active: Option<ActiveRecording>,
    /// Recordings whose worker is still finishing the MP4.
    finishing: Vec<mpsc::Receiver<Result<PathBuf, String>>>,
}

struct ActiveRecording {
    id: PreviewId,
    recorder: Recorder,
    clock: Clock,
    last_sent: Option<Instant>,
    /// Identity of the last frame handed over, so an unchanged source is
    /// not converted again; its picture simply holds in the video.
    last_source: usize,
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
    pub(super) fn toggle_recording(&mut self) {
        if self.recording.active.is_some() {
            self.stop_recording();
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
        let Some(directory) = self.storage.as_ref().map(Storage::recordings_dir) else {
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
                &format!("Could not create the recordings folder: {error}"),
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
            #[cfg(windows)]
            video_readback: None,
        });
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

    pub(super) fn stop_recording(&mut self) {
        let Some(active) = self.recording.active.take() else {
            return;
        };
        if let Some(preview) = self.preview_manager.get_mut(active.id) {
            preview.recording = false;
        }
        self.recording.finishing.push(active.recorder.stop());
        self.canvas_notice = Some(("Saving the recording…".to_owned(), Instant::now()));
    }

    /// Hand the recorded tile's newest frame to the encoder, at most
    /// `MAX_FPS` times a second. Runs after this frame's captures arrived.
    pub(super) fn recording_upkeep(&mut self, ctx: &egui::Context) {
        self.recording
            .finishing
            .retain(|receiver| match receiver.try_recv() {
                Ok(Ok(_)) => {
                    self.canvas_notice = Some((
                        "Recording saved to the Recordings folder.".to_owned(),
                        Instant::now(),
                    ));
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
            return self.stop_recording();
        };
        // Keep the timer ticking even while nothing else repaints.
        ctx.request_repaint_after(if active.clock.paused() {
            Duration::from_millis(500)
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

    pub(super) fn open_recordings_folder(&mut self) {
        let Some(directory) = self.storage.as_ref().map(Storage::recordings_dir) else {
            return;
        };
        if let Err(error) = std::fs::create_dir_all(&directory) {
            self.canvas_notice = Some((
                format!("Could not open the recordings folder: {error}"),
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
                    (
                        egui_phosphor::regular::RECORD,
                        egui::Color32::from_rgb(232, 72, 72),
                    )
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
            self.toggle_recording();
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
            self.toggle_recording();
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
        if crate::ui_theme::action(
            ui,
            egui_phosphor::regular::FOLDER_OPEN,
            "Open Recordings Folder",
        )
        .clicked()
        {
            self.open_recordings_folder();
            ui.close_menu();
        }
    }
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
