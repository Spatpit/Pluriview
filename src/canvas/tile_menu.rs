//! A tile's right-click menu. Every kind of tile lists the same sections in
//! the same order: what its content does, how it is viewed, where it sits,
//! its settings, then Remove.
use super::*;
use crate::preview::VideoSource;
use crate::ui_theme;
use egui_phosphor::regular as icons;

impl CanvasState {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn tile_menu(
        &mut self,
        ui: &mut egui::Ui,
        info: &TileInfo,
        canvas_rect: Rect,
        pixels_per_point: f32,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
        keyboard_shortcuts: &HotkeyBindings,
    ) {
        let id = info.id;
        let selected = if self.selection.contains(&id) {
            self.selection.clone()
        } else {
            vec![id]
        };
        ui.set_max_width(280.0);
        tile_menu_header(ui, info, preview_manager, selected.len());
        ui.separator();
        if self.is_spotlighting() {
            self.spotlight_menu(ui, canvas_rect, preview_manager);
            return;
        }

        // A frozen tile's own controls are unavailable until it resumes.
        if info.manually_frozen {
            self.activity_items(ui, &selected, preview_manager);
            ui.separator();
        }
        if self.content_items(
            ui,
            info,
            canvas_rect,
            pixels_per_point,
            preview_manager,
            keyboard_shortcuts,
        ) {
            ui.separator();
        }

        self.view_items(ui, info, canvas_rect, preview_manager, keyboard_shortcuts);
        ui.separator();

        self.arrange_menu(ui, id, canvas_rect, preview_manager, keyboard_shortcuts);
        self.group_menu(ui, id, canvas_rect, preview_manager, keyboard_shortcuts);
        self.spotlight_item(ui, id, canvas_rect, preview_manager, keyboard_shortcuts);
        ui.separator();

        self.tile_settings_menu(ui, info, canvas_rect, preview_manager);
        if !info.manually_frozen {
            self.activity_items(ui, &selected, preview_manager);
        }
        ui.separator();

        // Unlike Freeze, Remove only takes the tile that was clicked.
        let remove = if selected.len() > 1 {
            "Remove This Tile"
        } else {
            "Remove"
        };
        if ui_theme::destructive(ui, remove).clicked() {
            capture_coordinator.stop_capture(id);
            preview_manager.start_removal(id);
            self.selection.retain(|&x| x != id);
            ui.close_menu();
        }
    }

    /// Freeze or resume the tile, or every selected tile with it.
    fn activity_items(
        &mut self,
        ui: &mut egui::Ui,
        selected: &[PreviewId],
        preview_manager: &PreviewManager,
    ) {
        let frozen = |id: &PreviewId| preview_manager.get(*id).map(|p| p.manually_frozen);
        let any_frozen = selected.iter().any(|id| frozen(id) == Some(true));
        let any_live = selected.iter().any(|id| frozen(id) == Some(false));
        let several = selected.len() > 1;
        if any_frozen
            && ui
                .button(if several {
                    "Resume Selected"
                } else {
                    "Resume Tile"
                })
                .clicked()
        {
            self.queue_activity_for_selection(selected, TileActivityAction::Resume);
            ui.close_menu();
        }
        if any_live
            && ui
                .button(if several {
                    "Freeze Selected"
                } else {
                    "Freeze Tile"
                })
                .clicked()
        {
            self.queue_activity_for_selection(selected, TileActivityAction::Freeze);
            ui.close_menu();
        }
    }

    /// The actions of the tile's own content. Returns whether there are any.
    fn content_items(
        &mut self,
        ui: &mut egui::Ui,
        info: &TileInfo,
        canvas_rect: Rect,
        pixels_per_point: f32,
        preview_manager: &mut PreviewManager,
        keyboard_shortcuts: &HotkeyBindings,
    ) -> bool {
        if info.is_playlist {
            self.playlist_items(ui, info.id, preview_manager);
        } else if info.is_media {
            self.image_items(ui, info.id, preview_manager);
        } else if info.is_browser {
            self.browser_items(ui, info, preview_manager, keyboard_shortcuts);
        } else if info.is_video {
            self.video_items(ui, info);
        } else if info.is_window_capture {
            self.window_items(ui, info, canvas_rect, pixels_per_point, preview_manager);
        } else {
            return false;
        }
        true
    }

    fn browser_items(
        &mut self,
        ui: &mut egui::Ui,
        info: &TileInfo,
        preview_manager: &mut PreviewManager,
        keyboard_shortcuts: &HotkeyBindings,
    ) {
        let id = info.id;
        let ready = info.browser_status == BrowserTileStatus::Ready && !info.manually_frozen;
        if !ready && !info.manually_frozen {
            ui.label(egui::RichText::new("Browser preview is starting…").weak());
        }
        if ui
            .add_enabled(
                ready && !info.left_click_disabled,
                ui_theme::shortcut_button(
                    "Interact",
                    keyboard_shortcuts.get(HotkeySlot::InteractBrowser),
                ),
            )
            .on_disabled_hover_text(if info.left_click_disabled {
                "Turn off Disable Left Click in Tile Settings first"
            } else {
                "Available once the page has loaded"
            })
            .clicked()
        {
            self.last_double_clicked = Some(id);
            ui.close_menu();
        }
        if ui.add_enabled(ready, egui::Button::new("Reload")).clicked() {
            self.pending_browser_actions
                .push((id, BrowserAction::Reload));
            ui.close_menu();
        }
        if ui
            .add_enabled(
                ready,
                egui::Button::new(if info.muted { "Unmute" } else { "Mute" }),
            )
            .clicked()
        {
            self.pending_browser_actions
                .push((id, BrowserAction::ToggleMute));
            ui.close_menu();
        }
        page_zoom_row(ui, id, ready, preview_manager);

        ui.menu_button("Page", |ui| {
            ui_theme::menu(ui);
            for (label, action) in [
                ("Change URL…", BrowserAction::EditUrl),
                ("Copy URL", BrowserAction::CopyUrl),
                ("Open in Default Browser", BrowserAction::OpenExternal),
            ] {
                if ui.add_enabled(ready, egui::Button::new(label)).clicked() {
                    self.pending_browser_actions.push((id, action));
                    ui.close_menu();
                }
            }
            ui.separator();
            if ui
                .add_enabled(ready, egui::Button::new("Fit to Video"))
                .on_hover_text("Crop the tile to the page's video player")
                .clicked()
            {
                self.pending_browser_actions
                    .push((id, BrowserAction::FitToVideo));
                ui.close_menu();
            }
            let (mut show_frame, has_tabs) = preview_manager
                .get(id)
                .map(|preview| (preview.browser_chrome, preview.browser_tabs.len() > 1))
                .unwrap_or_default();
            let hint = if has_tabs && show_frame {
                "Hiding them shows only the current tab; the others stay open"
            } else {
                "Back, forward, an address bar and vertical tabs around the page"
            };
            if ui_theme::toggle(ui, &mut show_frame, "Show Toolbar and Tabs")
                .on_hover_text(hint)
                .clicked()
            {
                if let Some(preview) = preview_manager.get_mut(id) {
                    preview.browser_chrome = show_frame;
                }
                ui.close_menu();
            }
        });
    }

    fn video_items(&mut self, ui: &mut egui::Ui, info: &TileInfo) {
        let id = info.id;
        let playback = &info.video_playback;
        let controls_enabled = !info.manually_frozen
            && playback.connected
            && !matches!(info.video_status, VideoTileStatus::Failed(_));
        if !controls_enabled && !info.manually_frozen {
            ui.label(egui::RichText::new("Video controls are unavailable").weak());
        }
        ui.add_enabled_ui(controls_enabled, |ui| {
            if ui
                .button(if playback.paused { "Play" } else { "Pause" })
                .clicked()
            {
                self.pending_video_actions
                    .push((id, VideoAction::SetPaused(!playback.paused)));
                ui.close_menu();
            }
            if ui
                .button(if playback.muted { "Unmute" } else { "Mute" })
                .clicked()
            {
                self.pending_video_actions
                    .push((id, VideoAction::ToggleMute));
                ui.close_menu();
            }
            let mut volume = playback.volume;
            if ui
                .add(egui::Slider::new(&mut volume, 0.0..=100.0).text("Volume"))
                .changed()
            {
                self.pending_video_actions
                    .push((id, VideoAction::SetVolume(volume)));
            }
            ui.menu_button("Playback", |ui| {
                ui_theme::menu(ui);
                let mut looping = playback.looping;
                if ui_theme::toggle(ui, &mut looping, "Loop").clicked() {
                    self.pending_video_actions
                        .push((id, VideoAction::ToggleLoop));
                    ui.close_menu();
                }
                ui.menu_button(format!("Speed: {}×", playback.speed), |ui| {
                    ui_theme::menu(ui);
                    for speed in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0] {
                        let selected = (playback.speed - speed).abs() < 0.001;
                        if ui.selectable_label(selected, format!("{speed}×")).clicked() {
                            self.pending_video_actions
                                .push((id, VideoAction::SetSpeed(speed)));
                            ui.close_menu();
                        }
                    }
                });
                ui.menu_button("Audio Track", |ui| {
                    ui_theme::menu(ui);
                    let mut any = false;
                    for track in playback.tracks.iter().filter(|track| track.kind == "audio") {
                        any = true;
                        if ui
                            .selectable_label(
                                playback.audio_track == Some(track.id),
                                track_label(track),
                            )
                            .clicked()
                        {
                            self.pending_video_actions
                                .push((id, VideoAction::SelectAudioTrack(track.id)));
                            ui.close_menu();
                        }
                    }
                    if !any {
                        ui.label(egui::RichText::new("No audio tracks").weak());
                    }
                });
                ui.menu_button("Subtitles", |ui| {
                    ui_theme::menu(ui);
                    if ui
                        .selectable_label(playback.subtitle_track.is_none(), "Off")
                        .clicked()
                    {
                        self.pending_video_actions
                            .push((id, VideoAction::SelectSubtitleTrack(None)));
                        ui.close_menu();
                    }
                    for track in playback.tracks.iter().filter(|track| track.kind == "sub") {
                        if ui
                            .selectable_label(
                                playback.subtitle_track == Some(track.id),
                                track_label(track),
                            )
                            .clicked()
                        {
                            self.pending_video_actions
                                .push((id, VideoAction::SelectSubtitleTrack(Some(track.id))));
                            ui.close_menu();
                        }
                    }
                });
            });
        });
        if ui
            .add_enabled(!info.manually_frozen, egui::Button::new("Reload"))
            .clicked()
        {
            self.pending_video_actions.push((id, VideoAction::Reload));
            ui.close_menu();
        }
        if video_status_needs_settings(&info.video_status) && ui.button("Open Settings").clicked() {
            self.pending_video_actions
                .push((id, VideoAction::OpenSettings));
            ui.close_menu();
        }
    }

    fn playlist_items(
        &mut self,
        ui: &mut egui::Ui,
        id: PreviewId,
        preview_manager: &PreviewManager,
    ) {
        for (label, action) in [
            ("Previous Video", PlaylistAction::Previous),
            ("Next Video", PlaylistAction::Next),
        ] {
            if ui.button(label).clicked() {
                self.pending_playlist_actions.push((id, action));
                ui.close_menu();
            }
        }
        let (autoplay, shuffle, repeat) = preview_manager
            .get(id)
            .and_then(|preview| preview.folder_playlist.as_ref())
            .map(|playlist| (playlist.autoplay, playlist.shuffle, playlist.repeat))
            .unwrap_or((true, false, false));
        ui.menu_button("Playlist", |ui| {
            ui_theme::menu(ui);
            if ui.button("Rescan Folder").clicked() {
                self.pending_playlist_actions
                    .push((id, PlaylistAction::Rescan));
                ui.close_menu();
            }
            ui.separator();
            for (mut on, label, action) in [
                (autoplay, "Autoplay Next", PlaylistAction::ToggleAutoplay),
                (shuffle, "Shuffle", PlaylistAction::ToggleShuffle),
                (repeat, "Repeat Playlist", PlaylistAction::ToggleRepeat),
            ] {
                if ui_theme::toggle(ui, &mut on, label).clicked() {
                    self.pending_playlist_actions.push((id, action));
                    ui.close_menu();
                }
            }
        });
    }

    fn image_items(&mut self, ui: &mut egui::Ui, id: PreviewId, preview_manager: &PreviewManager) {
        if let Some(error) = preview_manager
            .get(id)
            .and_then(|preview| preview.capture_error.as_deref())
        {
            ui.label(egui::RichText::new(error).small().weak());
        }
        for (label, action) in [
            ("Retry Image", MediaAction::Retry),
            ("Locate Image…", MediaAction::Locate),
        ] {
            if ui.button(label).clicked() {
                self.pending_media_actions.push((id, action));
                ui.close_menu();
            }
        }
    }

    fn window_items(
        &mut self,
        ui: &mut egui::Ui,
        info: &TileInfo,
        canvas_rect: Rect,
        pixels_per_point: f32,
        preview_manager: &mut PreviewManager,
    ) {
        let connected = !info.is_inactive_window && !info.manually_frozen;
        if ui
            .add_enabled(
                connected && !info.left_click_disabled,
                egui::Button::new("Interact"),
            )
            .on_hover_text(
                "Use the app from this tile (or double-click it). Its window moves over the tile until you click elsewhere.",
            )
            .on_disabled_hover_text(if info.left_click_disabled {
                "Turn off Disable Left Click in Tile Settings first"
            } else {
                "Available while the app's window is connected"
            })
            .clicked()
        {
            self.last_double_clicked = Some(info.id);
            ui.close_menu();
        }
        if ui
            .add_enabled(
                !info.is_inactive_window,
                egui::Button::new("Show Original Window"),
            )
            .on_hover_text("Bring the app's own window to the front where it is")
            .clicked()
        {
            self.pending_show_window = Some(info.id);
            ui.close_menu();
        }
        if ui
            .add_enabled(
                info.source_frame_size.is_some(),
                egui::Button::new("Native Size (1:1)"),
            )
            .on_hover_text(
                "Resize this tile so every captured source pixel maps to one screen pixel",
            )
            .clicked()
        {
            if let Some(source_size) = info.source_frame_size {
                self.set_native_capture_size(
                    info.id,
                    source_size,
                    canvas_rect,
                    pixels_per_point,
                    preview_manager,
                );
            }
            ui.close_menu();
        }
    }

    /// How the tile is seen: brought into view, popped out or pinned.
    fn view_items(
        &mut self,
        ui: &mut egui::Ui,
        info: &TileInfo,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
        keyboard_shortcuts: &HotkeyBindings,
    ) {
        let id = info.id;
        if ui
            .add_enabled(
                info.viewport_pin.is_none(),
                ui_theme::shortcut_button(
                    "Focus on This Tile",
                    keyboard_shortcuts.get(HotkeySlot::FocusCurrentTile),
                ),
            )
            .on_disabled_hover_text("A pinned tile is always in view")
            .clicked()
        {
            self.focus_on_tile(id, info.rect, canvas_rect);
            ui.close_menu();
        }
        if !info.is_playlist {
            let mut popped_out = preview_manager
                .get(id)
                .is_some_and(|preview| preview.picture_in_picture.is_some());
            if ui_theme::shortcut_toggle(
                ui,
                &mut popped_out,
                "Picture in Picture",
                keyboard_shortcuts.get(HotkeySlot::PictureInPicture),
            )
            .on_hover_text(
                "Show this tile, or its whole group, in a small window that stays on top of other apps",
            )
            .clicked()
            {
                self.pending_picture_in_picture = Some(id);
                ui.close_menu();
            }
        }
        let mut pinned = info.viewport_pin.is_some();
        if ui_theme::toggle(ui, &mut pinned, "Pin to Viewport")
            .on_hover_text("Keep this tile fixed above the canvas while panning or zooming")
            .clicked()
        {
            self.toggle_viewport_pin(id, canvas_rect, preview_manager);
            ui.close_menu();
        }
    }

    /// Options set once and rarely changed.
    fn tile_settings_menu(
        &mut self,
        ui: &mut egui::Ui,
        info: &TileInfo,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
    ) {
        let id = info.id;
        ui.menu_button("Tile Settings", |ui| {
            ui_theme::menu(ui);
            if !info.is_playlist {
                self.crop_items(ui, id, canvas_rect, preview_manager);
                ui.separator();
            }
            if !info.is_media && !info.is_playlist {
                ui.menu_button(format!("Frame Rate: {}", info.fps_preset.label()), |ui| {
                    ui_theme::menu(ui);
                    for preset in [FpsPreset::Low, FpsPreset::Medium, FpsPreset::High] {
                        if ui
                            .selectable_label(info.fps_preset == preset, preset.label())
                            .clicked()
                        {
                            self.pending_fps_changes.push(PendingFpsChange {
                                preview_id: id,
                                new_fps: preset,
                            });
                            ui.close_menu();
                        }
                    }
                });
            }
            if info.is_browser {
                let mut locked = info.lock_aspect_ratio;
                if ui_theme::toggle(ui, &mut locked, "Lock Aspect Ratio")
                    .on_hover_text(
                        "Turn this off to resize the browser into portrait or any custom shape",
                    )
                    .clicked()
                {
                    self.toggle_aspect_ratio_lock(id, preview_manager);
                    ui.close_menu();
                }
            }
            let mut passes_through = info.left_click_disabled;
            if ui_theme::toggle(ui, &mut passes_through, "Disable Left Click")
                .on_hover_text("Let primary clicks and drags pass through to tiles underneath")
                .clicked()
            {
                self.toggle_left_click_disabled(id, preview_manager);
                ui.close_menu();
            }
        });
    }
}

/// The tile's name, and what kind of tile it is.
fn tile_menu_header(
    ui: &mut egui::Ui,
    info: &TileInfo,
    preview_manager: &PreviewManager,
    selected: usize,
) {
    let (icon, mut kind) = tile_kind(info, preview_manager);
    ui.label(egui::RichText::new(format!("{icon}  {}", compact_title(&info.title, 40))).strong());
    if selected > 1 {
        kind.push_str(&format!(" · {selected} tiles selected"));
    }
    ui.label(
        egui::RichText::new(kind)
            .size(12.0)
            .color(ui_theme::SECONDARY),
    );
}

fn tile_kind(info: &TileInfo, preview_manager: &PreviewManager) -> (&'static str, String) {
    let preview = preview_manager.get(info.id);
    let with_site = |kind: &str, url: Option<&str>| match url.map(browser_chrome::site_of) {
        Some(site) if !site.is_empty() => format!("{kind} · {site}"),
        _ => kind.to_owned(),
    };
    if info.is_playlist {
        let count = preview
            .and_then(|preview| preview.folder_playlist.as_ref())
            .map_or(0, |playlist| playlist.entries.len());
        let videos = if count == 1 {
            "1 video".to_owned()
        } else {
            format!("{count} videos")
        };
        (icons::PLAYLIST, format!("Folder Playlist · {videos}"))
    } else if info.is_media {
        (icons::IMAGE, "Image".to_owned())
    } else if info.is_browser {
        let url = preview.and_then(|preview| preview.browser_url.as_deref());
        (icons::GLOBE, with_site("Browser", url))
    } else if info.is_video {
        match preview.and_then(|preview| preview.video_source.as_ref()) {
            Some(VideoSource::Stream { url, .. }) => {
                (icons::BROADCAST, with_site("Stream", Some(url)))
            }
            _ => (icons::FILM_STRIP, "Video".to_owned()),
        }
    } else if info.is_spout_capture {
        (icons::BROADCAST, "Spout".to_owned())
    } else if info.is_window_capture || info.is_inactive_window {
        let kind = match &info.window_exe {
            Some(exe) => format!("Window · {exe}"),
            None => "Window".to_owned(),
        };
        (icons::APP_WINDOW, kind)
    } else {
        (icons::SQUARE, "Tile".to_owned())
    }
}

fn track_label(track: &crate::preview::VideoTrack) -> String {
    track
        .title
        .clone()
        .or_else(|| track.language.clone())
        .unwrap_or_else(|| format!("Track {}", track.id))
}

/// Page Zoom with − / reset / + buttons on one row.
fn page_zoom_row(
    ui: &mut egui::Ui,
    id: PreviewId,
    ready: bool,
    preview_manager: &mut PreviewManager,
) {
    let page_zoom = preview_manager
        .get(id)
        .map_or(1.0, |preview| preview.browser_page_zoom);
    let mut new_page_zoom = None;
    ui.horizontal(|ui| {
        ui.label("Page Zoom");
        if ui
            .add_enabled(
                ready && page_zoom > crate::browser::MIN_PAGE_ZOOM + 0.005,
                egui::Button::new(icons::MINUS),
            )
            .on_hover_text("Zoom out")
            .clicked()
        {
            new_page_zoom = Some(crate::browser::step_page_zoom(page_zoom, false));
        }
        if ui
            .add_enabled(
                ready,
                egui::Button::new(format!("{:.0}%", page_zoom * 100.0)),
            )
            .on_hover_text("Reset to 100%")
            .clicked()
        {
            new_page_zoom = Some(1.0);
        }
        if ui
            .add_enabled(
                ready && page_zoom < crate::browser::MAX_PAGE_ZOOM - 0.005,
                egui::Button::new(icons::PLUS),
            )
            .on_hover_text("Zoom in")
            .clicked()
        {
            new_page_zoom = Some(crate::browser::step_page_zoom(page_zoom, true));
        }
    });
    if let Some(zoom) = new_page_zoom {
        if let Some(preview) = preview_manager.get_mut(id) {
            preview.browser_page_zoom = zoom;
        }
    }
}
