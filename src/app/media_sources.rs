use super::*;

impl PluriviewApp {
    /// Reserve an image tile immediately and decode its original path off-thread.
    pub(super) fn import_media_tile(
        &mut self,
        source: &std::path::Path,
        position: Pos2,
    ) -> Result<PreviewId, String> {
        self.import_media_tile_sized(source, position, Vec2::new(640.0, 480.0))
    }

    /// Like `import_media_tile`, with a placeholder size; a size that already
    /// matches the image's proportions stays put when decoding finishes.
    pub(super) fn import_media_tile_sized(
        &mut self,
        source: &std::path::Path,
        position: Pos2,
        size: Vec2,
    ) -> Result<PreviewId, String> {
        let path = original_image_path(source)?;
        let title = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Image")
            .to_owned();
        let id = self
            .preview_manager
            .add_media(path.clone(), title, Vec::new(), position, size);
        self.queue_image_load(id, path, Some(size));
        Ok(id)
    }

    /// Recreate a tile from its original path. Relative filenames from older
    /// layouts still resolve through the legacy managed-media directory.
    pub(super) fn restore_media_tile(
        &mut self,
        saved_path: &std::path::Path,
        title: String,
        position: Pos2,
        size: Vec2,
    ) -> Result<PreviewId, String> {
        let path = self.resolve_saved_image_path(saved_path)?;
        let id = self
            .preview_manager
            .add_media(path.clone(), title, Vec::new(), position, size);
        self.queue_image_load(id, path, None);
        Ok(id)
    }

    pub(super) fn handle_media_action(&mut self, id: PreviewId, action: MediaAction) {
        let Some(path) = self
            .preview_manager
            .get(id)
            .filter(|preview| preview.is_media())
            .and_then(|preview| preview.media_path.clone())
        else {
            return;
        };
        let result = match action {
            MediaAction::Retry => self.resolve_saved_image_path(&path),
            MediaAction::Locate => {
                let Some(path) = media::pick_file(self.main_hwnd) else {
                    return;
                };
                original_image_path(&path)
            }
        };
        let path = match result {
            Ok(path) => path,
            Err(error) => {
                self.media_error = Some(error);
                return;
            }
        };
        if let Some(preview) = self.preview_manager.get_mut(id) {
            // Repairing a source must not move, unpin, uncrop, or unfreeze it.
            preview.set_media(path.clone(), Vec::new());
        }
        self.queue_image_load(id, path, None);
    }

    #[cfg(windows)]
    pub(super) fn restore_removed_preview(
        &mut self,
        info: RemovedPreviewInfo,
    ) -> Option<PreviewId> {
        let id = if let Some(playlist) = info.folder_playlist.as_ref() {
            let group = info.playlist_group?;
            let linked = self.preview_manager.all().find_map(|preview| {
                (preview.playlist_group == Some(group) && preview.is_video()).then_some(preview.id)
            });
            self.restore_folder_playlist(
                &playlist.layout(),
                info.title.clone(),
                info.position,
                info.size,
                group,
                linked,
            )
        } else if let Some(url) = info.browser_url.as_deref() {
            match self.create_browser_tile(url, info.position, info.size, info.fps_preset) {
                Ok(id) => {
                    self.apply_browser_mute(id, info.browser_muted);
                    id
                }
                Err(error) => {
                    self.media_error = Some(error);
                    return None;
                }
            }
        } else if let Some(media_path) = info.media_path.as_deref() {
            match self.restore_media_tile(media_path, info.title.clone(), info.position, info.size)
            {
                Ok(id) => id,
                Err(error) => {
                    self.media_error = Some(error);
                    return None;
                }
            }
        } else if let Some(source) = info.video_source.clone() {
            let id = self.create_video_tile(
                source,
                info.title.clone(),
                info.position,
                info.size,
                info.fps_preset,
                true,
            );
            let checkpoint = FrozenVideoCheckpoint {
                playback: info.video_playback.clone(),
                seek_position: resumable_video_position(&info.video_playback),
            };
            if info.manually_frozen {
                self.frozen_video_checkpoints.insert(id, checkpoint);
            } else {
                self.video_resume_checkpoints.insert(id, checkpoint);
            }
            id
        } else if let Some(sender) = info.spout_sender.as_ref() {
            let id = self.preview_manager.add_for_spout(
                sender.clone(),
                info.position,
                info.size,
                info.fps_preset,
            );
            if !info.manually_frozen {
                self.capture_coordinator.start_spout_capture(
                    id,
                    sender.clone(),
                    info.fps_preset.as_u32(),
                );
            }
            id
        } else if info.window_waiting_for_match || info.window_handle.is_some() {
            // HWNDs can be recycled while the Undo toast is visible. Reconnect
            // through the approved window list instead of trusting the old handle.
            self.preview_manager.add_inactive_window(
                info.title.clone(),
                info.window_exe.clone(),
                info.position,
                info.size,
                info.fps_preset,
                info.z_order,
            )
        } else {
            return None;
        };
        if let Some(preview) = self.preview_manager.get_mut(id) {
            restore_removed_tile_state(preview, &info);
        }
        Some(id)
    }

    #[cfg(windows)]
    pub(super) fn restore_removed_previews(&mut self, mut removed: Vec<RemovedPreviewInfo>) {
        let groups: HashSet<_> = removed
            .iter()
            .filter_map(|info| info.playlist_group)
            .collect();
        // Recreate video sources before rebuilding their playlist links.
        removed.sort_by_key(|info| (info.folder_playlist.is_some(), info.z_order));
        self.canvas.selection.clear();
        for info in removed {
            let selectable = !info.left_click_disabled;
            if let Some(id) = self.restore_removed_preview(info) {
                if selectable {
                    self.canvas.selection.push(id);
                }
            }
        }
        for group in groups {
            let linked = self.preview_manager.all().find_map(|preview| {
                (preview.playlist_group == Some(group) && preview.is_video()).then_some(preview.id)
            });
            for preview in self.preview_manager.all_mut() {
                if preview.is_playlist() && preview.playlist_group == Some(group) {
                    preview.playlist_linked_video = linked;
                }
            }
        }
    }

    pub(super) fn resolve_saved_image_path(
        &self,
        saved_path: &std::path::Path,
    ) -> Result<std::path::PathBuf, String> {
        if saved_path.is_absolute() {
            return Ok(saved_path.to_owned());
        }
        let filename = saved_path
            .to_str()
            .ok_or_else(|| "Saved legacy image path is invalid".to_owned())?;
        self.storage
            .as_ref()
            .ok_or_else(|| "Pluriview storage is unavailable".to_owned())?
            .resolve_media(filename)
            .ok_or_else(|| "Saved legacy image path is invalid".to_owned())
    }

    pub(super) fn pick_wallpaper(&mut self) {
        let Some(path) = media::pick_wallpaper_file(self.main_hwnd) else {
            return;
        };
        if let Err(error) = self.set_wallpaper_from_path(path) {
            log::error!("Failed to set wallpaper: {error}");
            self.media_error = Some(error);
        }
    }

    pub(super) fn clear_wallpaper(&mut self) {
        self.source_loads.cancel(SourceTarget::Wallpaper);
        #[cfg(windows)]
        {
            if self.video_manager.contains(WALLPAPER_VIDEO_ID) {
                self.video_manager.remove(WALLPAPER_VIDEO_ID);
            }
            self.wallpaper_sleep_since = None;
            self.wallpaper_hibernated = false;
        }
        self.canvas.wallpaper = None;
    }

    pub(super) fn set_wallpaper_from_path(
        &mut self,
        path: std::path::PathBuf,
    ) -> Result<(), String> {
        if media::is_supported_video_path(&path) {
            #[cfg(windows)]
            Self::require_libmpv("Setting a video wallpaper")?;
            self.set_video_wallpaper(path)
        } else if media::is_supported_image_path(&path) {
            self.set_image_wallpaper(&path)
        } else {
            Err(format!("Unsupported wallpaper file: {}", path.display()))
        }
    }

    pub(super) fn set_image_wallpaper(&mut self, source: &std::path::Path) -> Result<(), String> {
        let path = original_image_path(source)?;
        self.start_image_wallpaper(path)
    }

    pub(super) fn restore_image_wallpaper(
        &mut self,
        saved_path: &std::path::Path,
    ) -> Result<(), String> {
        let path = self.resolve_saved_image_path(saved_path)?;
        self.start_image_wallpaper(path)
    }

    pub(super) fn start_image_wallpaper(&mut self, path: std::path::PathBuf) -> Result<(), String> {
        self.clear_wallpaper();
        self.canvas.wallpaper = Some(CanvasWallpaper::from_image(path.clone(), Vec::new()));
        if let Err(error) =
            self.source_loads
                .start(SourceTarget::Wallpaper, move || PreparedSource::Image {
                    asset: media::load(&path),
                    path,
                    fit_size: None,
                })
        {
            self.source_load_failed(SourceTarget::Wallpaper, error);
        }
        Ok(())
    }

    #[cfg(windows)]
    pub(super) fn set_video_wallpaper(&mut self, path: std::path::PathBuf) -> Result<(), String> {
        self.clear_wallpaper();
        // Retain the source before attempting playback, even when unavailable.
        self.canvas.wallpaper = Some(CanvasWallpaper::from_video(path));
        match self.ensure_wallpaper_video() {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(wallpaper) = self.canvas.wallpaper.as_mut() {
                    wallpaper.error = Some(error.clone());
                }
                Err(error)
            }
        }
    }

    #[cfg(not(windows))]
    pub(super) fn set_video_wallpaper(&mut self, path: std::path::PathBuf) -> Result<(), String> {
        Err(format!(
            "Video wallpaper requires Windows and libmpv: {}",
            path.display()
        ))
    }

    pub(super) fn restore_wallpaper(&mut self, layout: WallpaperLayout) -> Result<(), String> {
        match layout {
            WallpaperLayout::Image { path } => self.restore_image_wallpaper(&path),
            WallpaperLayout::Video { path } => self.set_video_wallpaper(path),
        }
    }

    /// Attach libmpv to the current video wallpaper when the file is ready.
    #[cfg(windows)]
    pub(super) fn ensure_wallpaper_video(&mut self) -> Result<bool, String> {
        if self.wallpaper_hibernated || self.wallpaper_sleep_since.is_some() {
            return Ok(false);
        }
        let path = match self.canvas.wallpaper.as_ref() {
            Some(wallpaper) if wallpaper.video_renderer.is_none() => match &wallpaper.source {
                WallpaperSource::Video { path } => path.clone(),
                WallpaperSource::Image { .. } => return Ok(false),
            },
            _ => return Ok(false),
        };
        if let Some(tile) = self.video_manager.get(WALLPAPER_VIDEO_ID) {
            let renderer = tile.session.renderer();
            if let Some(wallpaper) = self.canvas.wallpaper.as_mut() {
                wallpaper.video_renderer = Some(renderer);
                wallpaper.video_sleeping = false;
            }
            return Ok(true);
        }
        if self
            .canvas
            .wallpaper
            .as_ref()
            .is_some_and(|wallpaper| wallpaper.error.is_some())
        {
            return Ok(false);
        }
        let source = VideoSource::LocalFile { path };
        let Some(mut launch) = video_launch_for_source(
            &source,
            self.external_tools.status(ToolKind::Streamlink),
            false,
        )?
        else {
            return Ok(false);
        };
        launch.wallpaper = true;
        let renderer =
            self.video_manager
                .launch(WALLPAPER_VIDEO_ID, launch, FpsPreset::Medium.as_u32())?;
        if let Some(tile) = self.video_manager.get_mut(WALLPAPER_VIDEO_ID) {
            let _ = tile.session.set_looping(true);
            let _ = tile.session.set_muted(true);
            let _ = tile.session.set_fill_frame(true);
        }
        if let Some(wallpaper) = self.canvas.wallpaper.as_mut() {
            wallpaper.video_renderer = Some(renderer);
            wallpaper.video_sleeping = false;
            wallpaper.error = None;
        }
        Ok(true)
    }

    /// Keep a muted video wallpaper from consuming decoder/GPU resources while
    /// the window cannot display it. This reuses normal libmpv cleanup and adds
    /// no polling thread; the existing egui upkeep tick drives the deadline.
    #[cfg(windows)]
    pub(super) fn wallpaper_memory_upkeep(&mut self, ctx: &egui::Context) {
        let is_video = self
            .canvas
            .wallpaper
            .as_ref()
            .is_some_and(|wallpaper| matches!(wallpaper.source, WallpaperSource::Video { .. }));
        if !is_video {
            self.wallpaper_sleep_since = None;
            self.wallpaper_hibernated = false;
            return;
        }

        let minimized = ctx.input(|input| input.viewport().minimized.unwrap_or(false));
        let hidden = minimized || self.canvas.is_focusing_tile();
        if !hidden {
            self.wallpaper_sleep_since = None;
            self.wallpaper_hibernated = false;
            if let Some(wallpaper) = self.canvas.wallpaper.as_mut() {
                wallpaper.video_sleeping = false;
            }
            return;
        }

        let now = Instant::now();
        let since = *self.wallpaper_sleep_since.get_or_insert(now);
        if let Some(wallpaper) = self.canvas.wallpaper.as_mut() {
            wallpaper.video_sleeping = true;
        }
        if let Err(error) = self.video_manager.set_paused(WALLPAPER_VIDEO_ID, true) {
            log::warn!("Could not pause minimized wallpaper: {error}");
        }
        if !self.wallpaper_hibernated && wallpaper_hibernation_due(since, now) {
            if let Some(wallpaper) = self.canvas.wallpaper.as_mut() {
                wallpaper.video_renderer = None;
            }
            self.video_manager.remove(WALLPAPER_VIDEO_ID);
            self.wallpaper_hibernated = true;
        }
    }

    /// Video wallpaper keeps decoding while focused tiles cover it. Pause the
    /// player (and skip painting) until tile focus is cleared.
    #[cfg(windows)]
    pub(super) fn sync_wallpaper_under_tile_focus(&mut self) {
        let covered = self.canvas.is_focusing_tile() || self.wallpaper_sleep_since.is_some();
        if let Err(error) = self.video_manager.set_paused(WALLPAPER_VIDEO_ID, covered) {
            log::warn!("Could not pause wallpaper under a focused tile: {error}");
        }
    }

    /// Open files dropped anywhere over the app and place them where the
    /// pointer meets the canvas. Images use managed storage; videos launch as
    /// MPV-backed tiles. Multiple files fan out so each remains selectable.
    pub(super) fn import_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }

        let Some(canvas_rect) = self.canvas.last_screen_rect else {
            return;
        };
        let pointer = ctx
            .input(|input| input.pointer.hover_pos())
            .unwrap_or_else(|| canvas_rect.center());
        let pointer = Pos2::new(
            pointer.x.clamp(canvas_rect.left(), canvas_rect.right()),
            pointer.y.clamp(canvas_rect.top(), canvas_rect.bottom()),
        );
        let base_position = self.canvas.screen_to_canvas(pointer, canvas_rect);
        let mut imported = Vec::new();
        let mut errors = Vec::new();

        for (index, file) in dropped.into_iter().enumerate() {
            let Some(path) = file.path else {
                let name = if file.name.is_empty() {
                    "dragged item"
                } else {
                    &file.name
                };
                errors.push(format!(
                    "{name}: this drag source did not provide a local file"
                ));
                continue;
            };
            let offset = Vec2::splat(index as f32 * 24.0 / self.canvas.zoom.max(0.1));
            let position = base_position + offset;
            let result: Result<Vec<PreviewId>, String> = if path.is_dir() {
                #[cfg(windows)]
                {
                    self.add_video_folder(path.clone(), position)
                        .map(|playlist| vec![playlist])
                }
                #[cfg(not(windows))]
                {
                    Err("Video playback is currently available only on Windows".to_owned())
                }
            } else if media::is_supported_video_path(&path) {
                #[cfg(windows)]
                {
                    self.add_local_video_path(path.clone(), position)
                        .map(|id| vec![id])
                }
                #[cfg(not(windows))]
                {
                    Err("Video playback is currently available only on Windows".to_owned())
                }
            } else {
                self.import_media_tile(&path, position).map(|id| vec![id])
            };
            match result {
                Ok(ids) => imported.extend(ids),
                Err(error) => errors.push(format!("{}: {error}", path.display())),
            }
        }

        if !imported.is_empty() {
            self.canvas.selection = imported;
        }
        if !errors.is_empty() {
            self.media_error = Some(errors.join("\n\n"));
        }
    }

    /// Highlight the canvas while Windows is carrying files over the app.
    pub(super) fn file_drop_overlay(&self, ctx: &egui::Context) {
        if !ctx.input(|input| !input.raw.hovered_files.is_empty()) {
            return;
        }
        let Some(rect) = self.canvas.last_screen_rect else {
            return;
        };
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("media_drop_overlay"),
        ));
        painter.rect_filled(
            rect,
            8.0,
            egui::Color32::from_rgba_unmultiplied(35, 95, 55, 90),
        );
        painter.rect_stroke(
            rect.shrink(8.0),
            8.0,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(107, 200, 110)),
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Drop images, videos, or a video folder onto the canvas",
            egui::FontId::proportional(18.0),
            egui::Color32::WHITE,
        );
        ctx.request_repaint();
    }
}
