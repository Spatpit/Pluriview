use super::*;

impl CanvasState {
    /// Cancel a gesture when the app switches to passing input through.
    pub fn cancel_pointer_interaction(&mut self) {
        self.groups.resize = None;
        self.groups.reorder = None;
        self.finish_crop();
        self.drag_state = None;
        self.marquee = None;
        self.preview_dragging = false;
        self.pinned_drag_origins.clear();
        self.pinned_pointer_drag = None;
        self.canvas_panning = false;
        self.pan_drag_tracker = DragTracker::new();
        self.last_secondary_click = None;
        self.last_double_clicked = None;
        self.interactive_browser = None;
        self.live_window.drag_cancelled();
    }

    pub fn set_keyboard_input(&mut self, input: CanvasKeyboardInput) {
        self.keyboard_input = Some(input);
    }

    /// Reset canvas to default view
    pub fn reset(&mut self) {
        self.groups.resize = None;
        self.finish_crop();
        self.pan = Vec2::ZERO;
        self.zoom = 1.0;
        self.selection.clear();
        self.drag_state = None;
        self.marquee = None;
        self.pinned_drag_origins.clear();
        self.pinned_pointer_drag = None;
        self.video_volume_hover = None;
        self.cancel_camera_transition();
    }

    /// Drop transient tile interaction state from a previous layout. Tile
    /// ids restart at 1 in the next layout, so anything still holding an old
    /// id (selection, focus, a gesture, a queued action) would otherwise act
    /// on an unrelated tile there.
    pub fn clear_preview_interactions(&mut self) {
        self.cancel_pointer_interaction();
        self.groups = GroupState::default();
        self.selection.clear();
        self.focus = None;
        self.browser_tab_scroll.clear();
        self.last_removed = None;
        self.removed_undo_requested = false;
        self.pending_removed_restore = None;
        self.pending_stream_audio_toggles.clear();
        self.pending_browser_actions.clear();
        self.pending_browser_scrolls.clear();
        self.pending_browser_navigations.clear();
        self.pending_video_actions.clear();
        self.pending_tile_activity_actions.clear();
        self.pending_playlist_actions.clear();
        self.pending_media_actions.clear();
        self.pending_show_window = None;
        self.pending_picture_in_picture = None;
        self.video_volume_hover = None;
        self.live_window = LiveWindow::default();
    }

    pub(super) fn prune_preview_interactions(&mut self, preview_manager: &PreviewManager) {
        if self.crop_mode.is_some_and(|id| {
            self.selection.as_slice() != [id]
                || !preview_manager.get(id).is_some_and(Self::can_edit_crop)
        }) {
            self.finish_crop();
        }
        if self
            .pinned_pointer_drag
            .is_some_and(|drag| preview_manager.get(drag.id).is_none())
        {
            self.pinned_pointer_drag = None;
        }
    }

    /// Fit one tile in the current canvas without changing its saved geometry.
    pub fn focus_on_tile(&mut self, id: PreviewId, tile_rect: Rect, canvas_rect: Rect) {
        self.groups.reorder = None;
        self.remember_current_tile(id);
        self.finish_crop();
        self.cancel_camera_transition();
        if let Some(focus) = &mut self.focus {
            focus.id = id;
        } else {
            self.focus = Some(FocusState {
                id,
                pan: self.pan,
                zoom: self.zoom,
                selection: self.selection.clone(),
            });
        }

        let available = canvas_rect.size().max(Vec2::splat(1.0));
        let tile_size = tile_rect.size().max(Vec2::splat(1.0));
        // Focus must also fit small companions beyond the ordinary navigation
        // zoom range. Leaving focus restores the previous camera unchanged.
        self.zoom = (available.x / tile_size.x).min(available.y / tile_size.y);
        self.pan =
            (canvas_rect.center() - canvas_rect.min) / self.zoom - tile_rect.center().to_vec2();
        self.selection = vec![id];
        self.animation.momentum_active = false;
        self.animation.momentum_velocity = Vec2::ZERO;
    }

    pub fn is_focusing_tile(&self) -> bool {
        self.focus.is_some()
    }

    pub fn focused_tile(&self) -> Option<PreviewId> {
        self.focus.as_ref().map(|focus| focus.id)
    }

    /// Hide tiles from the canvas or show them again. Their sources keep
    /// running; hidden tiles only leave the canvas, its selection and focus.
    pub fn set_tiles_hidden(
        &mut self,
        ids: &[PreviewId],
        hidden: bool,
        previews: &mut PreviewManager,
    ) {
        for id in ids {
            if let Some(preview) = previews.get_mut(*id) {
                preview.hidden = hidden;
            }
        }
        if !hidden {
            return;
        }
        if self
            .focus
            .as_ref()
            .is_some_and(|focus| ids.contains(&focus.id))
        {
            self.exit_focus();
        }
        if self.crop_mode.is_some_and(|id| ids.contains(&id)) {
            self.finish_crop();
        }
        self.selection.retain(|id| !ids.contains(id));
        if self
            .groups_active_tile()
            .is_some_and(|id| ids.contains(&id))
        {
            self.forget_current_tile();
        }
    }

    /// Bring hidden tiles back from the empty-canvas menu without opening
    /// the Control Tile Center. Hovering "Show Hidden Tiles" lists them by
    /// name to the side, with Show All when there are several.
    pub(super) fn show_hidden_items(&mut self, ui: &mut egui::Ui, previews: &mut PreviewManager) {
        let entries = Self::hidden_entries(previews);
        if entries.is_empty() {
            return;
        }
        let mut show = None;
        ui.menu_button("Show Hidden Tiles", |ui| {
            crate::ui_theme::menu(ui);
            for entry in &entries {
                if ui.button(&entry.label).clicked() {
                    show = Some(entry.members.clone());
                }
            }
            if entries.len() > 1 {
                ui.separator();
                if ui.button("Show All").clicked() {
                    show = Some(entries.iter().flat_map(|e| e.members.clone()).collect());
                }
            }
        });
        if let Some(ids) = show {
            self.set_tiles_hidden(&ids, false, previews);
            ui.close_menu();
        }
    }

    /// Hidden tiles in the user's list order, for the empty-canvas menu. A
    /// hidden group is one entry, as hiding one member hides them all.
    pub(super) fn hidden_entries(previews: &PreviewManager) -> Vec<HiddenEntry> {
        let mut hidden: Vec<_> = previews
            .all()
            .filter(|p| p.hidden && p.removing.is_none())
            .collect();
        hidden.sort_by_key(|p| (p.list_order, p.id.0));
        let mut entries = Vec::new();
        let mut groups_seen = Vec::new();
        for preview in hidden {
            let entry = match &preview.tile_group {
                Some(group) => {
                    if groups_seen.contains(&group.id) {
                        continue;
                    }
                    groups_seen.push(group.id);
                    let members: Vec<_> = Self::members_of(group.id, previews)
                        .into_iter()
                        .filter(|id| previews.get(*id).is_some_and(|p| p.hidden))
                        .collect();
                    let name = match group.name.trim() {
                        "" => "Group",
                        name => name,
                    };
                    HiddenEntry {
                        label: format!("{} · {} tiles", compact_title(name, 30), members.len()),
                        members,
                    }
                }
                None => HiddenEntry {
                    label: match preview.title.trim() {
                        "" => "Untitled Tile".to_owned(),
                        title => compact_title(title, 40),
                    },
                    members: vec![preview.id],
                },
            };
            entries.push(entry);
        }
        entries
    }

    /// Select `id` with its group, as a click on it would, showing it first
    /// when hidden.
    pub fn select_tile(&mut self, id: PreviewId, previews: &mut PreviewManager) {
        if previews.get(id).is_none_or(|p| p.removing.is_some()) {
            return;
        }
        let members = self.group_members(id, previews);
        if previews.get(id).is_some_and(|p| p.hidden) {
            self.set_tiles_hidden(&members, false, previews);
        }
        self.selection = members;
        self.remember_current_tile(id);
    }

    /// Pin a tile (with its group) to the viewport, or put it back on the
    /// canvas, as the tile menu does.
    pub fn toggle_tile_pin(&mut self, id: PreviewId, previews: &mut PreviewManager) {
        if let Some(viewport) = self.last_screen_rect {
            self.toggle_viewport_pin(id, viewport, previews);
        }
    }

    /// Where a tile is on the canvas right now. A pinned tile has no canvas
    /// place of its own, so this is the part of the canvas it covers in the
    /// current view.
    pub fn tile_canvas_rect(&self, preview: &Preview) -> Rect {
        match (preview.viewport_pin, self.last_screen_rect) {
            (Some(pin), Some(screen)) => {
                let rect = pin.rect(screen);
                Rect::from_min_max(
                    self.screen_to_canvas(rect.min, screen),
                    self.screen_to_canvas(rect.max, screen),
                )
            }
            _ => preview.rect(),
        }
    }

    /// Move a tile, with its group, by `delta` canvas units. A pinned tile
    /// moves to the matching place on screen, kept inside the window.
    pub fn move_tile_by(&mut self, id: PreviewId, delta: Vec2, previews: &mut PreviewManager) {
        if !delta.is_finite() || delta == Vec2::ZERO {
            return;
        }
        let members = self.group_members(id, previews);
        let screen = self.last_screen_rect;
        let pinned: Vec<(PreviewId, ViewportPin)> = members
            .iter()
            .filter_map(|member| previews.get(*member))
            .filter(|p| p.removing.is_none())
            .filter_map(|p| p.viewport_pin.map(|pin| (p.id, pin)))
            .collect();
        if let (Some(screen), false) = (screen, pinned.is_empty()) {
            // Pinned members move together, so clamp their shared bounds.
            let bounds = pinned
                .iter()
                .map(|(_, pin)| pin.rect(screen))
                .reduce(|a, b| a.union(b))
                .expect("pinned is not empty");
            let mut shift = delta * self.zoom;
            for axis in 0..2 {
                let (low, high) = (
                    screen.min[axis] - bounds.min[axis],
                    screen.max[axis] - bounds.max[axis],
                );
                // A block larger than the window keeps its top-left inside.
                shift[axis] = if low <= high {
                    shift[axis].clamp(low, high)
                } else {
                    low
                };
            }
            for (member, pin) in pinned {
                let moved =
                    self.moved_pin(member, pin.rect(screen).translate(shift), screen, previews);
                if let Some(preview) = previews.get_mut(member) {
                    preview.viewport_pin = Some(moved);
                }
            }
        }
        for member in members {
            if let Some(preview) = previews
                .get_mut(member)
                .filter(|p| p.removing.is_none() && p.viewport_pin.is_none())
            {
                preview.position += delta;
            }
        }
    }

    /// Resize an ungrouped pinned tile to `size` canvas units of the current
    /// view, keeping its top-left corner, as its bottom-right handle does:
    /// a locked aspect ratio is kept, as is the handles' minimum size, and
    /// the tile stays inside the window.
    pub fn resize_pinned_tile(&mut self, id: PreviewId, size: Vec2, previews: &mut PreviewManager) {
        let Some(screen) = self.last_screen_rect else {
            return;
        };
        let Some(preview) = previews
            .get_mut(id)
            .filter(|p| p.removing.is_none() && p.tile_group.is_none())
        else {
            return;
        };
        let Some(pin) = preview.viewport_pin.filter(|_| size.is_finite()) else {
            return;
        };
        let start = pin.rect(screen);
        let room = screen.max - start.min;
        let target = (size * self.zoom).min(room);
        let aspect_ratio = preview
            .lock_aspect_ratio
            .then_some(preview.source_aspect_ratio)
            .filter(|ratio| ratio.is_finite() && *ratio > 0.0);
        let rect = apply_resize(
            ResizeHandle::BottomRight,
            start,
            target - start.size(),
            aspect_ratio,
        );
        if rect.width() >= MIN_RESIZE_SIZE && rect.height() >= MIN_RESIZE_SIZE {
            preview.viewport_pin = Some(ViewportPin::from_rect(rect, screen));
        }
    }

    /// Turn a window tile's Stream Audio on or off on the next canvas frame,
    /// as its badge does.
    pub fn toggle_stream_audio(&mut self, id: PreviewId) {
        self.pending_stream_audio_toggles.push(id);
    }

    /// Change a tile's capture frame rate on the next canvas frame.
    pub fn set_tile_fps(&mut self, id: PreviewId, fps: FpsPreset) {
        self.pending_fps_changes.push(PendingFpsChange {
            preview_id: id,
            new_fps: fps,
        });
    }

    /// Center the view on a canvas point at the current zoom.
    pub fn look_at(&mut self, point: Pos2, animate: bool, now: f64) {
        let Some(rect) = self.last_screen_rect else {
            return;
        };
        if animate {
            self.start_camera_transition(point, self.zoom, rect, now);
            return;
        }
        self.focus = None;
        self.cancel_camera_transition();
        self.animation.momentum_active = false;
        self.animation.momentum_velocity = Vec2::ZERO;
        self.pan = (rect.center() - rect.min) / self.zoom - point.to_vec2();
    }

    /// Fit `id` in the canvas, or return from it when it is already focused.
    /// A hidden tile is shown first; a pinned one is only selected.
    pub fn toggle_tile_focus(&mut self, id: PreviewId, previews: &mut PreviewManager) {
        if self.focused_tile() == Some(id) {
            self.exit_focus();
            return;
        }
        let Some(preview) = previews.get(id).filter(|p| p.removing.is_none()) else {
            return;
        };
        let (pinned, rect) = (preview.viewport_pin.is_some(), preview.rect());
        if preview.hidden {
            let members = self.group_members(id, previews);
            self.set_tiles_hidden(&members, false, previews);
        }
        match self.last_screen_rect {
            Some(viewport) if !pinned => self.focus_on_tile(id, rect, viewport),
            _ => {
                self.exit_focus();
                self.selection = vec![id];
                self.remember_current_tile(id);
            }
        }
    }

    /// Remote focus uses canvas hit testing, never tile clicks or source input.
    pub fn focus_viewer_at(&mut self, at: [f32; 2], previews: &PreviewManager) {
        let Some(rect) = self.last_screen_rect else {
            return;
        };
        if !at.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
            return;
        }
        let position = rect.min + rect.size() * egui::vec2(at[0], at[1]);
        let Some(id) = self.preview_at_screen(position, rect, previews) else {
            return;
        };
        let Some(preview) = previews.get(id).filter(|p| p.viewport_pin.is_none()) else {
            return;
        };
        if self.focus.as_ref().is_some_and(|focus| focus.id == id) {
            self.exit_focus();
        } else {
            self.focus_on_tile(id, preview.rect(), rect);
        }
    }

    /// Canvas-space point currently shown at the center of the viewport.
    pub fn camera_center(&self, canvas_rect: Rect) -> Pos2 {
        self.screen_to_canvas(canvas_rect.center(), canvas_rect)
    }

    /// Camera-only navigation shared by remote viewers. It never synthesizes
    /// pointer events, so dragging over content cannot move or activate a tile.
    pub fn apply_viewer_camera(&mut self, command: crate::remote_view::CameraCommand) {
        let Some(rect) = self.last_screen_rect else {
            return;
        };
        if !command.is_valid() || !rect.is_positive() {
            return;
        }
        let anchor = rect.min + rect.size() * egui::vec2(command.anchor[0], command.anchor[1]);
        let canvas_anchor = self.screen_to_canvas(anchor, rect);
        self.focus = None;
        self.cancel_camera_transition();
        self.animation.momentum_active = false;
        self.animation.momentum_velocity = Vec2::ZERO;
        self.zoom = (self.zoom * command.scale).clamp(self.zoom_min, self.zoom_max);
        self.pan = (anchor - rect.min) / self.zoom - canvas_anchor.to_vec2()
            + rect.size() * egui::vec2(command.pan[0], command.pan[1]) / self.zoom;
    }

    /// Glide to a named view. A new request samples the camera's current
    /// position, so selecting another view mid-transition remains continuous.
    pub fn start_camera_transition(
        &mut self,
        target_center: Pos2,
        target_zoom: f32,
        canvas_rect: Rect,
        now: f64,
    ) {
        let start_center = self.camera_center(canvas_rect);
        let target_zoom = target_zoom.clamp(self.zoom_min, self.zoom_max);
        self.focus = None;
        self.animation.momentum_active = false;
        self.animation.momentum_velocity = Vec2::ZERO;

        if start_center.distance(target_center) < 0.01 && (self.zoom - target_zoom).abs() < 0.0001 {
            self.zoom = target_zoom;
            self.pan =
                (canvas_rect.center() - canvas_rect.min) / self.zoom - target_center.to_vec2();
            self.animation.camera_transition = None;
            return;
        }

        self.animation.camera_transition = Some(CameraTransition::new(
            start_center,
            target_center,
            self.zoom,
            target_zoom,
            canvas_rect.size(),
            now,
        ));
    }

    /// Stop a saved-view glide at its current position. Direct navigation
    /// input calls this before applying its own movement.
    pub fn cancel_camera_transition(&mut self) {
        self.animation.camera_transition = None;
    }

    pub fn is_camera_transitioning(&self) -> bool {
        self.animation.camera_transition.is_some()
    }

    pub(super) fn update_camera_transition(&mut self, canvas_rect: Rect, now: f64) {
        let Some(sample) = self
            .animation
            .camera_transition
            .as_ref()
            .map(|transition| transition.sample(now))
        else {
            return;
        };

        self.zoom = sample.zoom.clamp(self.zoom_min, self.zoom_max);
        self.pan = (canvas_rect.center() - canvas_rect.min) / self.zoom - sample.center.to_vec2();
        if sample.finished {
            self.animation.camera_transition = None;
        }
    }

    /// Restore the canvas view saved before tile focus.
    pub fn exit_focus(&mut self) -> bool {
        let Some(focus) = self.focus.take() else {
            return false;
        };
        self.pan = focus.pan;
        self.zoom = focus.zoom;
        self.selection = focus.selection;
        true
    }

    pub(super) fn refit_focus(&mut self, preview_manager: &PreviewManager, canvas_rect: Rect) {
        let Some(id) = self.focus.as_ref().map(|focus| focus.id) else {
            return;
        };
        if let Some(tile_rect) = preview_manager
            .get(id)
            .filter(|preview| {
                preview.removing.is_none() && !preview.hidden && preview.viewport_pin.is_none()
            })
            .map(|preview| preview.rect())
        {
            self.focus_on_tile(id, tile_rect, canvas_rect);
        } else {
            self.exit_focus();
            self.selection.retain(|selected| *selected != id);
        }
    }

    /// Convert screen position to canvas position
    pub fn screen_to_canvas(&self, screen_pos: Pos2, canvas_rect: Rect) -> Pos2 {
        let relative = screen_pos - canvas_rect.min;
        Pos2::new(
            (relative.x / self.zoom) - self.pan.x,
            (relative.y / self.zoom) - self.pan.y,
        )
    }

    /// Convert canvas position to screen position
    pub fn canvas_to_screen(&self, canvas_pos: Pos2, canvas_rect: Rect) -> Pos2 {
        Pos2::new(
            (canvas_pos.x + self.pan.x) * self.zoom + canvas_rect.min.x,
            (canvas_pos.y + self.pan.y) * self.zoom + canvas_rect.min.y,
        )
    }

    /// Convert canvas rect to screen rect
    pub fn canvas_rect_to_screen(&self, canvas_rect: Rect, screen_canvas_rect: Rect) -> Rect {
        let min = self.canvas_to_screen(canvas_rect.min, screen_canvas_rect);
        let max = self.canvas_to_screen(canvas_rect.max, screen_canvas_rect);
        Rect::from_min_max(min, max)
    }

    /// Where a tile was last shown on screen, for overlays drawn by the app.
    pub fn tile_screen_rect(&self, id: PreviewId, previews: &PreviewManager) -> Option<Rect> {
        let canvas_rect = self.last_screen_rect?;
        let preview = previews.get(id)?;
        Some(self.preview_screen_rect(preview, canvas_rect))
    }

    /// Tiles a capture action applies to: the focused tile, else the selection.
    pub fn capture_targets(&self) -> Vec<PreviewId> {
        match &self.focus {
            Some(focus) => vec![focus.id],
            None => self.selection.clone(),
        }
    }

    pub(super) fn preview_screen_rect(&self, preview: &Preview, canvas_rect: Rect) -> Rect {
        let target = preview
            .viewport_pin
            .map(|pin| pin.rect(canvas_rect))
            .unwrap_or_else(|| self.canvas_rect_to_screen(preview.rect(), canvas_rect));
        self.layout_screen_rect(preview.id, target, canvas_rect)
    }

    pub(super) fn can_edit_crop(preview: &Preview) -> bool {
        !preview.is_playlist()
            && !preview.left_click_disabled
            && preview.removing.is_none()
            && (preview.source_frame_size.or(preview.frame_size).is_some() || preview.is_video())
    }

    pub(super) fn begin_crop(&mut self, id: PreviewId, preview_manager: &PreviewManager) {
        if !preview_manager.get(id).is_some_and(Self::can_edit_crop) {
            return;
        }
        self.exit_focus();
        self.cancel_camera_transition();
        self.selection = vec![id];
        self.crop_mode = Some(id);
        self.animation.momentum_active = false;
        self.animation.momentum_velocity = Vec2::ZERO;
        self.drag_state = None;
        self.marquee = None;
        self.preview_dragging = false;
        self.pinned_drag_origins.clear();
        self.pinned_pointer_drag = None;
        if self.interactive_browser == Some(id) {
            self.last_double_clicked = Some(id);
            self.interactive_browser = None;
        }
    }

    pub(super) fn finish_crop(&mut self) {
        if let Some(id) = self.crop_mode.take() {
            if matches!(self.drag_state, Some(DragState::Cropping { id: drag_id, .. }) if id == drag_id)
            {
                self.drag_state = None;
            }
        }
    }

    pub(super) fn crop_items(
        &mut self,
        ui: &mut egui::Ui,
        id: PreviewId,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
    ) {
        let editing = self.crop_mode == Some(id);
        let can_crop = preview_manager.get(id).is_some_and(Self::can_edit_crop);
        if ui
            .add_enabled(
                can_crop,
                egui::Button::new(if editing {
                    "Finish Cropping"
                } else {
                    "Edit Crop"
                }),
            )
            .on_hover_text("Alt+drag edges or corners also crops")
            .clicked()
        {
            if editing {
                self.finish_crop();
            } else {
                self.begin_crop(id, preview_manager);
            }
            ui.close_menu();
        }
        let has_crop = preview_manager
            .get(id)
            .is_some_and(|preview| preview.crop_uv.is_some());
        if ui
            .add_enabled(has_crop, egui::Button::new("Clear Crop"))
            .clicked()
        {
            self.set_preview_crop(id, None, canvas_rect, preview_manager);
            ui.close_menu();
        }
    }

    /// Apply or clear crop UVs while changing the tile bounds by the same
    /// source-space proportion, preserving content scale for Clear Crop and
    /// handle dragging.
    pub fn set_preview_crop(
        &mut self,
        id: PreviewId,
        crop_uv: Option<(f32, f32, f32, f32)>,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
    ) {
        let Some((current_crop, current_rect, screen_space, source_size)) =
            preview_manager.get(id).map(|preview| {
                let screen_space = preview.viewport_pin.is_some();
                (
                    preview.crop_uv.unwrap_or((0.0, 0.0, 1.0, 1.0)),
                    if screen_space {
                        self.preview_screen_rect(preview, canvas_rect)
                    } else {
                        preview.rect()
                    },
                    screen_space,
                    preview.source_frame_size.or(preview.frame_size),
                )
            })
        else {
            return;
        };
        let target_crop = crop_uv.unwrap_or((0.0, 0.0, 1.0, 1.0));
        // A browser's toolbar and tabs are not part of its page: crop from
        // the page alone, and frame the page again once the crop is cleared.
        let (framed, collapsed) = preview_manager
            .get(id)
            .map(|preview| {
                (
                    chrome_geometry::shows_chrome(preview),
                    preview.browser_tabs_collapsed,
                )
            })
            .unwrap_or_default();
        let current_rect = if framed && crop_uv.is_some() {
            chrome_geometry::strip_chrome(current_rect, collapsed)
        } else {
            current_rect
        };
        let new_rect = rect_for_crop_change(current_crop, target_crop, current_rect);
        let wants_frame = crop_uv.is_none()
            && preview_manager
                .get(id)
                .is_some_and(|preview| preview.is_browser() && preview.browser_chrome);
        let new_rect = if wants_frame && !framed {
            chrome_geometry::wrap_with_chrome(new_rect, collapsed)
        } else {
            new_rect
        };
        if let Some(preview) = preview_manager.get_mut(id) {
            preview.crop_uv = crop_uv;
            if screen_space {
                preview.viewport_pin = Some(groups::pin_for_tile(preview, new_rect, canvas_rect));
            } else {
                preview.position = new_rect.min;
                preview.size = new_rect.size();
            }
            if let Some((width, height)) = source_size {
                let cropped_width = (target_crop.2 - target_crop.0) * width as f32;
                let cropped_height = (target_crop.3 - target_crop.1) * height as f32;
                if cropped_height > 0.0 {
                    preview.source_aspect_ratio = cropped_width / cropped_height;
                }
            } else if new_rect.height() > 0.0 {
                preview.source_aspect_ratio = new_rect.width() / new_rect.height();
            }
        }
    }

    /// Shrink a browser tile showing its toolbar and tabs to just its page,
    /// so a crop gesture starting at `rect` (tile units) acts on the page.
    /// Returns the rect the gesture should start from.
    pub(super) fn drop_browser_frame_for_crop(
        &self,
        id: PreviewId,
        rect: Rect,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
    ) -> Rect {
        let Some(preview) = preview_manager
            .get_mut(id)
            .filter(|preview| chrome_geometry::shows_chrome(preview))
        else {
            return rect;
        };
        let page = chrome_geometry::strip_chrome(rect, preview.browser_tabs_collapsed);
        if preview.viewport_pin.is_some() {
            preview.viewport_pin = Some(groups::pin_for_tile(preview, page, canvas_rect));
        } else {
            preview.position = page.min;
            preview.size = page.size();
        }
        page
    }

    pub(super) fn toggle_single_viewport_pin(
        &mut self,
        id: PreviewId,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
    ) {
        let Some((pin, canvas_preview_rect)) = preview_manager
            .get(id)
            .map(|preview| (preview.viewport_pin, preview.rect()))
        else {
            return;
        };

        self.exit_focus();
        self.pinned_drag_origins
            .retain(|(drag_id, _)| *drag_id != id);
        if self.pinned_pointer_drag.is_some_and(|drag| drag.id == id) {
            self.pinned_pointer_drag = None;
        }

        if let Some(pin) = pin {
            let screen_rect = pin.rect(canvas_rect);
            let min = self.screen_to_canvas(screen_rect.min, canvas_rect);
            let max = self.screen_to_canvas(screen_rect.max, canvas_rect);
            if let Some(preview) = preview_manager.get_mut(id) {
                preview.position = min;
                preview.size = (max - min).max(Vec2::splat(1.0));
                preview.viewport_pin = None;
            }
        } else {
            let screen_rect = self.canvas_rect_to_screen(canvas_preview_rect, canvas_rect);
            if let Some(preview) = preview_manager.get_mut(id) {
                preview.viewport_pin = Some(ViewportPin::from_rect(screen_rect, canvas_rect));
            }
        }
    }

    pub(super) fn toggle_left_click_disabled(
        &mut self,
        id: PreviewId,
        preview_manager: &mut PreviewManager,
    ) {
        let Some(disabled) = preview_manager.get_mut(id).map(|preview| {
            preview.left_click_disabled = !preview.left_click_disabled;
            preview.left_click_disabled
        }) else {
            return;
        };

        if disabled {
            if self.focus.as_ref().is_some_and(|focus| focus.id == id) {
                self.exit_focus();
            }
            self.selection.retain(|selected| *selected != id);
            self.pinned_drag_origins
                .retain(|(drag_id, _)| *drag_id != id);
            if self.pinned_pointer_drag.is_some_and(|drag| drag.id == id) {
                self.pinned_pointer_drag = None;
            }
            if self.interactive_browser == Some(id) {
                // The app consumes this runtime request and parks the native
                // WebView host so it cannot continue intercepting pointer input.
                self.last_double_clicked = Some(id);
            }
        }
    }

    pub(super) fn toggle_aspect_ratio_lock(
        &mut self,
        id: PreviewId,
        preview_manager: &mut PreviewManager,
    ) {
        let Some(preview) = preview_manager.get_mut(id) else {
            return;
        };
        preview.lock_aspect_ratio = !preview.lock_aspect_ratio;
        if preview.lock_aspect_ratio {
            let size = preview
                .viewport_pin
                .map(ViewportPin::size_vec2)
                .unwrap_or(preview.size);
            if size.y > 0.0 {
                preview.source_aspect_ratio = size.x / size.y;
            }
        }
    }

    pub(super) fn set_native_capture_size(
        &mut self,
        id: PreviewId,
        source_size: (u32, u32),
        canvas_rect: Rect,
        pixels_per_point: f32,
        preview_manager: &mut PreviewManager,
    ) {
        // Focus mode owns the canvas zoom, so restore the user's normal view
        // before calculating a persistent 1:1 tile size.
        self.exit_focus();
        self.release_partial_groups(&[id], preview_manager);

        let Some((screen_rect, crop_uv, is_pinned)) = preview_manager.get(id).map(|preview| {
            (
                self.preview_screen_rect(preview, canvas_rect),
                preview.crop_uv,
                preview.viewport_pin.is_some(),
            )
        }) else {
            return;
        };
        let aligned_min = pixel_aligned_rect(
            Rect::from_min_size(screen_rect.min, Vec2::ZERO),
            pixels_per_point,
        )
        .min;
        let size = native_capture_canvas_size(
            source_size,
            crop_uv,
            pixels_per_point,
            if is_pinned { 1.0 } else { self.zoom },
        );

        if let Some(preview) = preview_manager.get_mut(id) {
            if is_pinned {
                preview.viewport_pin = Some(groups::pin_for_tile(
                    preview,
                    Rect::from_min_size(aligned_min, size),
                    canvas_rect,
                ));
            } else {
                let position = self.screen_to_canvas(aligned_min, canvas_rect);
                preview.position = position;
                preview.size = size;
            }
        }
    }

    /// Get the visible canvas area
    pub fn get_viewport(&self, screen_rect: Rect) -> Rect {
        let min = self.screen_to_canvas(screen_rect.min, screen_rect);
        let max = self.screen_to_canvas(screen_rect.max, screen_rect);
        Rect::from_min_max(min, max)
    }

    /// Check if mouse is over a resize handle, returns (preview_id, handle)
    pub(super) fn get_handle_at(
        &self,
        screen_pos: Pos2,
        canvas_rect: Rect,
        preview_manager: &PreviewManager,
        alt: bool,
    ) -> Option<(PreviewId, ResizeHandle)> {
        if self.has_selected_group(preview_manager) && !alt {
            return self.group_handle_at(screen_pos, canvas_rect, preview_manager);
        }
        for id in &self.selection {
            if let Some(preview) = preview_manager.get(*id) {
                if preview.left_click_disabled {
                    continue;
                }
                let screen_rect = self.preview_screen_rect(preview, canvas_rect);

                let handles = [
                    (screen_rect.left_top(), ResizeHandle::TopLeft),
                    (screen_rect.center_top(), ResizeHandle::Top),
                    (screen_rect.right_top(), ResizeHandle::TopRight),
                    (screen_rect.left_center(), ResizeHandle::Left),
                    (screen_rect.right_center(), ResizeHandle::Right),
                    (screen_rect.left_bottom(), ResizeHandle::BottomLeft),
                    (screen_rect.center_bottom(), ResizeHandle::Bottom),
                    (screen_rect.right_bottom(), ResizeHandle::BottomRight),
                ];

                for (handle_pos, handle_type) in handles {
                    let handle_rect =
                        Rect::from_center_size(handle_pos, Vec2::splat(RESIZE_HANDLE_HIT_SIZE));
                    if handle_rect.contains(screen_pos) {
                        return Some((*id, handle_type));
                    }
                }
            }
        }
        None
    }

    pub(super) fn preview_at_screen(
        &self,
        screen_pos: Pos2,
        canvas_rect: Rect,
        preview_manager: &PreviewManager,
    ) -> Option<PreviewId> {
        preview_manager
            .all()
            .filter(|preview| {
                preview.removing.is_none()
                    && !preview.hidden
                    && self
                        .preview_screen_rect(preview, canvas_rect)
                        .contains(screen_pos)
            })
            .max_by_key(|preview| (preview.viewport_pin.is_some(), preview.z_order))
            .map(|preview| preview.id)
    }

    /// The browser tile under `screen_pos` and that point in normalized page
    /// coordinates, through the tile's crop. None for other tiles and for
    /// browser tiles that are frozen or not running yet.
    pub(super) fn browser_page_point_at(
        &self,
        screen_pos: Pos2,
        canvas_rect: Rect,
        preview_manager: &PreviewManager,
    ) -> Option<(PreviewId, Vec2)> {
        let id = self.left_click_preview_at_screen(screen_pos, canvas_rect, preview_manager)?;
        let preview = preview_manager.get(id)?;
        if !preview.is_browser()
            || preview.manually_frozen
            || preview.browser_status != BrowserTileStatus::Ready
        {
            return None;
        }
        let rect =
            chrome_geometry::page_rect_for(preview, self.preview_screen_rect(preview, canvas_rect));
        if rect.width() <= 0.0 || rect.height() <= 0.0 || !rect.contains(screen_pos) {
            return None;
        }
        let (min_u, min_v, max_u, max_v) = preview.crop_uv.unwrap_or((0.0, 0.0, 1.0, 1.0));
        let t = (screen_pos - rect.min) / rect.size();
        Some((
            id,
            Vec2::new(min_u + t.x * (max_u - min_u), min_v + t.y * (max_v - min_v)),
        ))
    }

    pub(super) fn left_click_preview_at_screen(
        &self,
        screen_pos: Pos2,
        canvas_rect: Rect,
        preview_manager: &PreviewManager,
    ) -> Option<PreviewId> {
        preview_manager
            .all()
            .filter(|preview| {
                preview.removing.is_none()
                    && !preview.hidden
                    && !preview.left_click_disabled
                    && self
                        .preview_screen_rect(preview, canvas_rect)
                        .contains(screen_pos)
            })
            .max_by_key(|preview| (preview.viewport_pin.is_some(), preview.z_order))
            .map(|preview| preview.id)
    }

    pub(super) fn handle_pinned_pointer_drag(
        &mut self,
        id: PreviewId,
        _pin: ViewportPin,
        screen_rect: Rect,
        frame: CanvasFrameScope<'_>,
        preview_manager: &mut PreviewManager,
    ) -> bool {
        let CanvasFrameScope {
            canvas_rect,
            input,
            show_overlays,
            keyboard_shortcuts: _,
        } = frame;
        if input.pointer_blocked
            || self.groups.reorder.is_some()
            || self.groups.resize.is_some()
            || self.crop_mode == Some(id)
            || preview_manager
                .get(id)
                .is_some_and(|preview| preview.left_click_disabled)
        {
            return false;
        }
        if let Some(drag) = self.pinned_pointer_drag.filter(|drag| drag.id == id) {
            if let Some(pointer) = input.hover_pos {
                for &(member, origin) in &self.pinned_drag_origins {
                    let moved_rect = origin
                        .rect(canvas_rect)
                        .translate(pointer - drag.start_pointer);
                    let pin = self.moved_pin(member, moved_rect, canvas_rect, preview_manager);
                    if let Some(preview) = preview_manager.get_mut(member) {
                        preview.viewport_pin = Some(pin);
                    }
                }
                for member in &self.selection {
                    if let Some(preview) = preview_manager
                        .get_mut(*member)
                        .filter(|p| p.viewport_pin.is_none())
                    {
                        preview.translate(input.pointer_delta / self.zoom);
                    }
                }
            }
            if input.primary_released || !input.primary_down {
                self.pinned_pointer_drag = None;
            }
            return true;
        }

        let Some(pointer) = input.interact_pos.filter(|pointer| {
            input.primary_pressed
                && !input.alt
                && !input.middle_down
                && screen_rect.contains(*pointer)
                && !self.live_window.covers(*pointer)
        }) else {
            return false;
        };
        if self.left_click_preview_at_screen(pointer, canvas_rect, preview_manager) != Some(id) {
            return false;
        }
        // Tabs drag within their strip instead of moving the tile.
        if preview_manager
            .get(id)
            .is_some_and(|preview| self.over_tab_list(preview, screen_rect, pointer))
        {
            return false;
        }
        let over_resize_handle = show_overlays
            && self
                .get_handle_at(pointer, canvas_rect, preview_manager, input.alt)
                .is_some();
        if over_resize_handle {
            return false;
        }

        // Ctrl toggles selection; Shift+drag swaps places in a group.
        if input.ctrl || (input.shift && self.is_arranged(id, preview_manager)) {
            return false;
        }
        self.remember_current_tile(id);
        if !self.selection.contains(&id) {
            self.select_group_tile(id, preview_manager);
        }
        let selection = self.selection.clone();
        self.release_partial_groups(&selection, preview_manager);
        self.pinned_drag_origins = self
            .selection
            .iter()
            .filter_map(|id| {
                preview_manager
                    .get(*id)
                    .and_then(|p| p.viewport_pin.map(|pin| (*id, pin)))
            })
            .collect();
        self.pinned_pointer_drag = Some(PinnedPointerDrag {
            id,
            start_pointer: pointer,
        });
        true
    }
}

/// One line of the empty-canvas menu's hidden-tile list.
#[derive(Debug, PartialEq)]
pub(super) struct HiddenEntry {
    pub label: String,
    pub members: Vec<PreviewId>,
}
