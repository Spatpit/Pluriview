use super::*;

impl PluriviewApp {
    /// Keep source editing out of a failed-load canvas until the user recovers
    /// it or explicitly opens a different workspace.
    pub(super) fn workspace_recovery_ui(&mut self, ctx: &egui::Context) {
        self.handle_frameless_resize(ctx);
        egui::TopBottomPanel::top("workspace_recovery_title").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let title = ui.label("Pluriview — Workspace Recovery");
                if title.interact(egui::Sense::drag()).drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Minimize").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                });
            });
        });
        let mut retry = false;
        let mut recover = false;
        let mut create = false;
        let mut switch = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(24.0);
                ui.heading("This workspace could not be opened");
                ui.label("Editing and saving are paused to protect your saved files.");
                ui.add_space(12.0);
                if let Some(error) = self
                    .workspace_error
                    .as_deref()
                    .or(self.workspace_save.load_error())
                {
                    ui.label(error);
                }
                ui.add_space(16.0);
                retry = ui.button("Retry Load").clicked();
                if self.workspace_persistence_ready {
                    let backup_available = self.storage.as_ref().is_some_and(|storage| {
                        storage.workspace_backup_exists(&self.workspaces.active_workspace_id)
                    });
                    recover = ui
                        .add_enabled(
                            backup_available,
                            egui::Button::new("Restore Previous Save as New Workspace"),
                        )
                        .clicked();
                    create = ui.button("Open New Workspace…").clicked();
                    ui.add_space(16.0);
                    for workspace in &self.workspaces.workspaces {
                        if workspace.id != self.workspaces.active_workspace_id
                            && ui.button(format!("Open {}", workspace.name)).clicked()
                        {
                            switch = Some(workspace.id.clone());
                        }
                    }
                }
            });
        });
        if retry {
            if !self.workspace_persistence_ready {
                if let Some(storage) = &self.storage {
                    match storage.load_or_initialize_workspaces() {
                        Ok(index) => {
                            self.workspaces = index;
                            self.workspace_persistence_ready = true;
                        }
                        Err(error) => self.workspace_error = Some(error.to_string()),
                    }
                }
            }
            self.load_active_workspace();
        } else if recover {
            if let Err(error) = self.restore_previous_workspace_save() {
                self.workspace_error = Some(error);
            }
        } else if let Some(id) = switch {
            if let Err(error) = self.switch_workspace(&id) {
                self.workspace_error = Some(error);
            }
        } else if create {
            self.workspace_dialog = Some(WorkspaceDialog {
                kind: WorkspaceDialogKind::Create,
                name: "Untitled Workspace".to_owned(),
                focused: false,
            });
        }
        self.workspace_dialog_ui(ctx);
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    /// Load the active named workspace if it has been saved before.
    pub(super) fn load_active_workspace(&mut self) {
        if !self.workspace_persistence_ready {
            return;
        }
        let Some(storage) = &self.storage else {
            return;
        };
        let id = self.workspaces.active_workspace_id.clone();
        match self.workspace_save.load(storage, &id) {
            Ok(layout) => {
                self.apply_layout(&layout);
                self.remember_saved_workspace(&layout);
                #[cfg(debug_assertions)]
                println!(
                    "Loaded workspace {id} with {} previews",
                    layout.previews.len()
                );
            }
            Err(error) => {
                self.workspace_error = Some(error);
            }
        }
    }

    /// Save the current workspace and mirror it to legacy autosave.json.
    pub(super) fn save_active_workspace(&mut self) -> Result<(), String> {
        if !self.workspace_persistence_ready {
            return Err(
                "Workspace saving is disabled because the catalog could not be loaded safely. Restart after repairing the workspace files."
                    .to_owned(),
            );
        }
        let Some(storage) = &self.storage else {
            return Err("Workspace storage is unavailable.".to_owned());
        };
        let layout = self.create_layout();
        self.workspace_save
            .save(storage, &self.workspaces, &layout)?;
        self.last_workspace_autosave_check = Instant::now();
        #[cfg(debug_assertions)]
        println!(
            "Saved workspace {} with {} previews",
            self.workspaces.active_workspace_id,
            layout.previews.len()
        );
        Ok(())
    }

    pub(super) fn remember_saved_workspace(&mut self, layout: &SavedLayout) {
        self.workspace_save.remember(layout);
        self.workspace_error = None;
        self.last_workspace_autosave_check = Instant::now();
    }

    pub(super) fn workspace_autosave_upkeep(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_workspace_autosave_check);
        if elapsed < WORKSPACE_AUTOSAVE_INTERVAL {
            ctx.request_repaint_after(WORKSPACE_AUTOSAVE_INTERVAL - elapsed);
            return;
        }
        self.last_workspace_autosave_check = now;
        if !self.workspace_persistence_ready || !self.workspace_save.is_ready() {
            return;
        }
        if let Err(error) = self.save_active_workspace() {
            self.workspace_error = Some(error);
        }
    }

    pub(super) fn save_before_workspace_change(&mut self) -> Result<(), String> {
        if !self.workspace_persistence_ready {
            return Err("The workspace catalog must be recovered first.".to_owned());
        }
        if self.workspace_save.is_ready() {
            self.save_active_workspace()?;
        }
        Ok(())
    }

    pub(super) fn blank_workspace_layout(&self) -> SavedLayout {
        let mut layout = self.create_layout();
        layout.canvas = CanvasLayout::default();
        layout.previews.clear();
        layout
    }

    pub(super) fn switch_workspace(&mut self, id: &str) -> Result<(), String> {
        if id == self.workspaces.active_workspace_id {
            return Ok(());
        }
        if self
            .workspaces
            .workspaces
            .iter()
            .all(|workspace| workspace.id != id)
        {
            return Err("That workspace no longer exists.".to_owned());
        }
        self.save_before_workspace_change()?;

        let layout = self
            .storage
            .as_ref()
            .ok_or_else(|| "Workspace storage is unavailable.".to_owned())?
            .load_workspace(id)
            .map_err(|error| format!("Could not load workspace: {error}"))?;
        let previous_id =
            std::mem::replace(&mut self.workspaces.active_workspace_id, id.to_owned());
        if let Some(storage) = &self.storage {
            if let Err(error) = storage.save_workspace_index(&self.workspaces) {
                self.workspaces.active_workspace_id = previous_id;
                return Err(format!("Could not select workspace: {error}"));
            }
            if let Err(error) = storage.save_autosave(&layout) {
                self.workspaces.active_workspace_id = previous_id;
                let _ = storage.save_workspace_index(&self.workspaces);
                return Err(format!(
                    "Could not update the compatibility autosave: {error}"
                ));
            }
        }
        self.apply_layout(&layout);
        self.remember_saved_workspace(&layout);
        Ok(())
    }

    pub(super) fn create_workspace(&mut self, name: &str, duplicate: bool) -> Result<(), String> {
        let name = self.valid_workspace_name(name, None)?;
        if duplicate && !self.workspace_save.is_ready() {
            return Err("Load or recover the workspace before duplicating it.".to_owned());
        }
        self.save_before_workspace_change()?;
        let layout = if duplicate {
            self.create_layout()
        } else {
            self.blank_workspace_layout()
        };
        let previous_index = self.workspaces.clone();
        let id = self.workspaces.add(name);
        self.workspaces.active_workspace_id = id;

        let Some(storage) = &self.storage else {
            self.workspaces = previous_index;
            return Err("Workspace storage is unavailable.".to_owned());
        };
        if let Err(error) = storage.save_active_workspace(&self.workspaces, &layout) {
            self.workspaces = previous_index;
            return Err(format!("Could not create workspace: {error}"));
        }
        self.apply_layout(&layout);
        self.remember_saved_workspace(&layout);
        Ok(())
    }

    pub(super) fn rename_active_workspace(&mut self, name: &str) -> Result<(), String> {
        let active_id = self.workspaces.active_workspace_id.clone();
        let name = self.valid_workspace_name(name, Some(&active_id))?;
        let previous_index = self.workspaces.clone();
        if !self.workspaces.rename(&active_id, name) {
            return Err("The active workspace no longer exists.".to_owned());
        }
        let result = self
            .storage
            .as_ref()
            .ok_or_else(|| "Workspace storage is unavailable.".to_owned())?
            .save_workspace_index(&self.workspaces);
        if let Err(error) = result {
            self.workspaces = previous_index;
            return Err(format!("Could not rename workspace: {error}"));
        }
        Ok(())
    }

    pub(super) fn delete_active_workspace(&mut self) -> Result<(), String> {
        if self.workspaces.workspaces.len() <= 1 {
            return Err("The only workspace cannot be deleted.".to_owned());
        }
        let deleted_id = self.workspaces.active_workspace_id.clone();
        let previous_index = self.workspaces.clone();
        if !self.workspaces.remove(&deleted_id) {
            return Err("The active workspace could not be removed.".to_owned());
        }
        let next_id = self.workspaces.active_workspace_id.clone();
        let Some(storage) = &self.storage else {
            self.workspaces = previous_index;
            return Err("Workspace storage is unavailable.".to_owned());
        };
        let layout = match storage.load_workspace(&next_id) {
            Ok(layout) => layout,
            Err(error) => {
                self.workspaces = previous_index;
                return Err(format!("Could not open the next workspace: {error}"));
            }
        };
        if let Err(error) = storage.save_workspace_index(&self.workspaces) {
            self.workspaces = previous_index;
            return Err(format!("Could not update the workspace list: {error}"));
        }
        if let Err(error) = storage.save_autosave(&layout) {
            self.workspaces = previous_index;
            let _ = storage.save_workspace_index(&self.workspaces);
            return Err(format!(
                "Could not update the compatibility autosave: {error}"
            ));
        }
        let deletion_error = storage.delete_workspace(&deleted_id).err().map(|error| {
            format!(
                "The workspace was removed from the list, but its old file could not be deleted: {error}"
            )
        });
        self.apply_layout(&layout);
        self.remember_saved_workspace(&layout);
        self.workspace_error = deletion_error;
        Ok(())
    }

    pub(super) fn restore_previous_workspace_save(&mut self) -> Result<(), String> {
        if !self.workspace_persistence_ready {
            return Err(
                "Workspace saving is unavailable until the catalog is repaired.".to_owned(),
            );
        }
        let active_id = self.workspaces.active_workspace_id.clone();
        let active_name = self
            .workspaces
            .active()
            .map(|workspace| workspace.name.clone())
            .unwrap_or_else(|| "Workspace".to_owned());
        let recovered_layout = self
            .storage
            .as_ref()
            .ok_or_else(|| "Workspace storage is unavailable.".to_owned())?
            .load_workspace_backup(&active_id)
            .map_err(|error| format!("Could not load the previous workspace save: {error}"))?;

        // Preserve the current state first. The old backup is already in memory,
        // so rotating the current primary cannot destroy the recovery candidate.
        self.save_before_workspace_change()?;
        let previous_index = self.workspaces.clone();
        let recovered_name = self.unique_recovered_workspace_name(&active_name);
        let recovered_id = self.workspaces.add(recovered_name);
        self.workspaces.active_workspace_id = recovered_id;
        let save_result = self
            .storage
            .as_ref()
            .expect("workspace storage checked above")
            .save_active_workspace(&self.workspaces, &recovered_layout);
        if let Err(error) = save_result {
            self.workspaces = previous_index;
            return Err(format!("Could not create the recovered workspace: {error}"));
        }
        self.apply_layout(&recovered_layout);
        self.remember_saved_workspace(&recovered_layout);
        Ok(())
    }

    pub(super) fn unique_recovered_workspace_name(&self, active_name: &str) -> String {
        let shortened = active_name.chars().take(38).collect::<String>();
        let base = format!("{shortened} Recovered");
        if self
            .workspaces
            .workspaces
            .iter()
            .all(|workspace| !workspace.name.eq_ignore_ascii_case(&base))
        {
            return base;
        }
        for suffix in 2.. {
            let candidate = format!("{shortened} Recovered {suffix}");
            if self
                .workspaces
                .workspaces
                .iter()
                .all(|workspace| !workspace.name.eq_ignore_ascii_case(&candidate))
            {
                return candidate;
            }
        }
        unreachable!("an unused recovered workspace name always exists")
    }

    pub(super) fn valid_workspace_name(
        &self,
        name: &str,
        except_id: Option<&str>,
    ) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Enter a workspace name.".to_owned());
        }
        if name.chars().count() > 60 {
            return Err("Workspace names can contain at most 60 characters.".to_owned());
        }
        if self.workspaces.workspaces.iter().any(|workspace| {
            Some(workspace.id.as_str()) != except_id && workspace.name.eq_ignore_ascii_case(name)
        }) {
            return Err("A workspace with that name already exists.".to_owned());
        }
        Ok(name.to_owned())
    }

    /// Mirror the window's geometry into [`Self::window_layout`]. Minimized
    /// and maximized windows report the geometry they currently occupy, not
    /// the one to reopen at, so only the restored size and position are kept.
    pub(super) fn remember_window_geometry(&mut self, ctx: &egui::Context) {
        let (position, size, maximized, minimized) = ctx.input(|i| {
            let viewport = i.viewport();
            (
                viewport.outer_rect.map(|rect| (rect.min.x, rect.min.y)),
                viewport
                    .inner_rect
                    .map(|rect| (rect.width(), rect.height())),
                viewport.maximized.unwrap_or(false),
                viewport.minimized.unwrap_or(false),
            )
        });
        if minimized {
            return;
        }

        self.window_layout.maximized = maximized;
        if maximized {
            return;
        }
        if let Some(position) = position {
            self.window_layout.position = Some(position);
        }
        if let Some(size) = size {
            self.window_layout.size = size;
        }
    }

    /// Create a SavedLayout from current state
    pub(super) fn create_layout(&self) -> SavedLayout {
        let mut layout = SavedLayout::new();

        // Save canvas state
        layout.canvas = CanvasLayout {
            pan: (self.canvas.pan.x, self.canvas.pan.y),
            zoom: self.canvas.zoom,
            show_grid: self.canvas.show_grid,
            wallpaper: self
                .canvas
                .wallpaper
                .as_ref()
                .map(CanvasWallpaper::to_layout),
            views: self.canvas_views.clone(),
            active_view: self.active_canvas_view,
        };

        // Save all previews
        layout.previews = self
            .preview_manager
            .all()
            .map(|p| {
                let mut saved = self.canvas.saved_tile_layout(p);
                #[cfg(windows)]
                {
                    // Ask the live WebView where it actually is, so the tile
                    // reopens on the last page browsed rather than the one it
                    // was created with. Falls back to the mirrored URL for
                    // tiles whose host is gone.
                    saved.browser_url = self
                        .browser
                        .get(p.id)
                        .and_then(|host| host.live_url())
                        .or(saved.browser_url)
                        .as_deref()
                        .and_then(scrub_url_for_storage);
                }
                saved
            })
            .collect();

        layout.recent_browser_urls = self.recent_urls.clone();
        layout.picker_open = self.picker_open;
        layout.window = Some(self.window_layout);
        #[cfg(windows)]
        {
            layout.monitor_device = self.monitor_device.clone();
            layout.adblock_enabled = self.browser.adblock_enabled();
        }

        layout
    }

    /// Apply a SavedLayout to restore state
    pub(super) fn apply_layout(&mut self, layout: &SavedLayout) {
        self.source_loads.clear();
        self.canvas_view_dialog = None;
        self.active_canvas_view = None;
        self.deleted_canvas_view = None;
        self.clear_wallpaper();
        // Clear existing state
        self.capture_coordinator.stop_all();
        #[cfg(windows)]
        {
            self.video_manager.clear();
            self.pending_video_tiles.clear();
            self.last_restored_video_start = None;
            self.restored_paused_videos.clear();
            self.pending_video_freezes.clear();
            self.frozen_video_checkpoints.clear();
            self.video_resume_checkpoints.clear();
            self.seek_preview_manager.clear();
            self.playlist_thumbnail_queue.clear();
            self.playlist_thumbnail_jobs.clear();
            self.next_playlist_group = layout
                .previews
                .iter()
                .filter_map(|preview| preview.playlist_group)
                .max()
                .unwrap_or(0)
                .saturating_add(1);
        }
        self.preview_manager.clear();
        self.canvas.clear_preview_interactions();
        #[cfg(windows)]
        {
            self.browser.clear();
            self.pending_browser_tiles.clear();
            self.last_restored_browser_start = None;
            self.audio_monitor = None;
            self.audio_monitor_checked = None;
            self.window_audio_monitors.clear();
            self.last_inactive_window_scan = None;
        }

        // Restore canvas state
        self.canvas.pan = Vec2::new(layout.canvas.pan.0, layout.canvas.pan.1);
        self.canvas.zoom = layout.canvas.zoom;
        self.canvas.show_grid = layout.canvas.show_grid;
        self.canvas.cancel_camera_transition();
        self.canvas_views = layout.canvas.views.clone();
        self.active_canvas_view = layout
            .canvas
            .active_view
            .filter(|index| *index < self.canvas_views.len());
        if let Some(wallpaper) = layout.canvas.wallpaper.clone() {
            if let Err(error) = self.restore_wallpaper(wallpaper) {
                log::error!("Failed to restore wallpaper: {error}");
                self.media_error = Some(error);
            }
        }
        self.picker_open = layout.picker_open;
        // The window is not moved from here — main.rs opens it at the saved
        // geometry, and reloading a layout mid-session should not move the
        // window out from under the user. Mirroring the value only keeps
        // `pending_maximize` (read once, in `new`) in sync.
        self.window_layout = layout.window.unwrap_or(self.window_layout);

        #[cfg(windows)]
        {
            self.recent_urls = layout
                .recent_browser_urls
                .iter()
                .filter_map(|url| scrub_url_for_storage(url))
                .collect();
        }
        #[cfg(not(windows))]
        {
            self.recent_urls = layout.recent_browser_urls.clone();
        }
        #[cfg(windows)]
        {
            self.monitor_device = layout.monitor_device.clone();
            if let Err(error) = self.browser.set_adblock_enabled(layout.adblock_enabled) {
                log::error!("Could not restore ad blocker state: {error}");
            }
        }

        // Enumerate current windows to find matching ones
        let current_windows = enumerate_windows();
        let mut claimed_hwnds = HashSet::new();

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
                        if let Some(pending) = self.pending_browser_tiles.get_mut(&id) {
                            pending.restore_deferred = true;
                        }
                        self.preview_manager.set_z_order(id, preview_layout.z_order);
                        if let Some(preview) = self.preview_manager.get_mut(id) {
                            restore_browser_geometry(preview, preview_layout);
                        }
                        self.apply_browser_mute(id, preview_layout.browser_muted);
                    }
                    Err(error) => {
                        log::error!("Failed to restore browser tile: {error}");
                    }
                }
                continue;
            }

            if let Some(media_path) = &preview_layout.media_path {
                match self.restore_media_tile(
                    media_path,
                    preview_layout.window_title.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                ) {
                    Ok(id) => {
                        self.preview_manager.set_z_order(id, preview_layout.z_order);
                        if let Some(preview) = self.preview_manager.get_mut(id) {
                            restore_layout_tile_state(preview, preview_layout);
                        }
                    }
                    Err(error) => {
                        log::error!("Failed to restore image tile: {error}");
                        self.media_error = Some(error);
                    }
                }
                continue;
            }

            #[cfg(windows)]
            if let Some(folder_playlist) = &preview_layout.folder_playlist {
                let group = preview_layout.playlist_group.unwrap_or_else(|| {
                    let group = self.next_playlist_group;
                    self.next_playlist_group = self.next_playlist_group.saturating_add(1);
                    group
                });
                let id = self.restore_folder_playlist(
                    folder_playlist,
                    preview_layout.window_title.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    group,
                    None,
                );
                self.preview_manager.set_z_order(id, preview_layout.z_order);
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    restore_layout_tile_state(preview, preview_layout);
                }
                continue;
            }

            #[cfg(windows)]
            if let Some(source) = &preview_layout.video_source {
                let id = self.create_video_tile(
                    source.clone(),
                    preview_layout.window_title.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    preview_layout.fps_preset,
                    true,
                );
                self.preview_manager.set_z_order(id, preview_layout.z_order);
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    restore_layout_tile_state(preview, preview_layout);
                }
                continue;
            }

            if let Some(sender) = &preview_layout.spout_sender {
                let id = self.preview_manager.add_for_spout(
                    sender.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    preview_layout.fps_preset,
                );
                self.preview_manager.set_z_order(id, preview_layout.z_order);
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    restore_layout_tile_state(preview, preview_layout);
                }
                if !preview_layout.manually_frozen {
                    self.capture_coordinator.start_spout_capture(
                        id,
                        sender.clone(),
                        preview_layout.fps_preset.as_u32(),
                    );
                }
                continue;
            }

            // Prefer the exact saved title, but use the executable identity
            // when dynamic titles (Spotify tracks, documents, etc.) changed.
            let matching_window = find_saved_window(
                &preview_layout.window_title,
                preview_layout.window_exe.as_deref(),
                &current_windows,
                &claimed_hwnds,
            );

            if let Some(window_info) = matching_window {
                claimed_hwnds.insert(window_info.hwnd);
                // Create preview with saved position/size
                let id = self.preview_manager.add_with_window(
                    window_info.title.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    WindowHandle {
                        hwnd: window_info.hwnd,
                        process_id: window_info.process_id,
                    },
                    preview_layout.fps_preset,
                    preview_layout.z_order,
                );

                // Restore crop region if it was saved
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    preview.window_exe = Some(window_info.exe_name.clone());
                    preview.window_waiting_for_match = false;
                    restore_layout_tile_state(preview, preview_layout);
                }

                if !preview_layout.manually_frozen {
                    self.capture_coordinator.start_capture(
                        id,
                        window_info.hwnd,
                        window_info.title.clone(),
                        preview_layout.fps_preset.as_u32(),
                    );
                }

                #[cfg(debug_assertions)]
                println!(
                    "Restored preview: {}",
                    privacy::redact_title(&window_info.title)
                );
            } else {
                let id = self.preview_manager.add_inactive_window(
                    preview_layout.window_title.clone(),
                    preview_layout.window_exe.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    preview_layout.fps_preset,
                    preview_layout.z_order,
                );
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    restore_layout_tile_state(preview, preview_layout);
                }
                #[cfg(debug_assertions)]
                println!(
                    "Window inactive; keeping saved tile: {}",
                    privacy::redact_title(&preview_layout.window_title)
                );
            }
        }

        #[cfg(windows)]
        {
            let group_to_video = self
                .preview_manager
                .all()
                .filter_map(|preview| {
                    preview
                        .playlist_group
                        .filter(|_| preview.is_video())
                        .map(|group| (group, preview.id))
                })
                .collect::<HashMap<_, _>>();
            for preview in self.preview_manager.all_mut() {
                if preview.is_playlist() {
                    preview.playlist_linked_video = preview
                        .playlist_group
                        .and_then(|group| group_to_video.get(&group).copied());
                }
            }
        }
    }
}
