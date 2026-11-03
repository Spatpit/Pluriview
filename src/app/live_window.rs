//! Using a window tile's app in place. Double-clicking the tile frames it at
//! the window's real size, then moves the real window exactly over it and
//! hands it the keyboard. Clicking anywhere outside the app, Done or the exit
//! shortcut puts the window back where it was, with Pluriview in front of it
//! again if it was before. See `crate::window_interaction`.
//!
//! While the window is over its tile, the pointer is over that window, so a
//! tool capturing Pluriview (Discord, OBS) does not draw it. The tile's own
//! capture draws it instead; the tile lines up with the window, so the
//! pointer shows exactly where it is. The app's menus and dialogs are
//! separate windows, so the tile's capture takes those in too where Windows
//! can, and runs at least at 60 FPS so the pointer moves smoothly.
//!
//! A crash would leave the window over where the tile was, so while it is in
//! use a small file in the data folder records where it came from; the next
//! launch puts it back if the user has not touched it since.
use super::*;
use crate::window_interaction::{self as native, SavedPlacement};

/// How long the window has to take the foreground before a missing handover
/// counts as the user having moved on.
const FOCUS_GRACE: Duration = Duration::from_millis(600);
/// A move the app never finishes stops being awaited after this long.
const MOVE_TIMEOUT: Duration = Duration::from_millis(800);
/// Polling while the window is in use: often enough to notice a click
/// elsewhere, rarely enough to keep an idle canvas cheap.
const IDLE_POLL: Duration = Duration::from_millis(100);
/// Polling while the window is being dragged or moved, so its tile follows.
const ACTIVE_POLL: Duration = Duration::from_millis(16);
const ACTIVE_AFTER_MOVE: Duration = Duration::from_millis(500);
/// Monitor scale changes the tile is re-fitted for before giving up.
const MAX_REFITS: u8 = 4;
/// After Pluriview moves the window, a size change this soon is the app
/// adapting to the new monitor rather than the user resizing it.
const SETTLE_TIME: Duration = Duration::from_secs(1);
/// The tile's capture rate while in use, unless it is set higher.
const IN_USE_FPS: FpsPreset = FpsPreset::High;
/// How often a window moving around updates the crash-recovery file.
const RECOVERY_SAVE_INTERVAL: Duration = Duration::from_millis(500);

/// The window tile currently in use. Runtime-only.
pub(super) struct LiveWindowSession {
    id: PreviewId,
    hwnd: isize,
    pid: u32,
    /// Outer top-left before Pluriview moved the window.
    home: (i32, i32),
    /// Outer top-left after the last move, Pluriview's or the user's.
    known: (i32, i32),
    /// The window was minimized and goes back to that layout.
    minimized_from: Option<SavedPlacement>,
    /// Pluriview covered the window before, and covers it again after.
    was_below: bool,
    /// Other apps' windows that covered it, nearest first; it goes back
    /// under them.
    covered_by: Vec<isize>,
    /// Visible frame size the tile is framed for.
    visible_size: (i32, i32),
    /// Scale of the monitor the window is expected on.
    dpi: u32,
    /// Canvas zoom the window was placed at; None for pinned tiles.
    placed_zoom: Option<f32>,
    refits: u8,
    phase: Phase,
    /// When the user last dragged the window, to poll quickly meanwhile.
    moved_at: Option<Instant>,
    /// When Pluriview's last move of the window completed.
    placed_at: Option<Instant>,
    /// Where the crash-recovery file last recorded the window, and when.
    recorded: Option<((i32, i32), Instant)>,
}

/// The crash-recovery file: enough to put a window back where it came from,
/// and to tell whether it has been touched since.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct WindowInUse {
    hwnd: isize,
    pid: u32,
    /// Outer top-left to return the window to.
    home: (i32, i32),
    /// Outer top-left Pluriview last left it at.
    placed: (i32, i32),
    /// It goes back minimized.
    minimized: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// The camera glides to show the tile at the window's real size.
    Framing,
    /// Waiting for the app to move its window; `raise` then brings it in
    /// front. An app adapting to a new monitor's scale may land elsewhere,
    /// so a change from `size` also ends the wait.
    Moving {
        target: (i32, i32),
        size: (i32, i32),
        since: Instant,
        raise: bool,
    },
    /// The window is over its tile. `since` starts the focus grace period.
    Live { since: Instant },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    /// Put the window back where it was.
    Restore,
    /// The user resized, maximized or snapped the window: leave it there.
    Leave,
    /// The window closed or hid itself.
    Gone,
}

impl PluriviewApp {
    /// Double-click or Interact on a window tile.
    pub(super) fn toggle_live_window(&mut self, ctx: &egui::Context, id: PreviewId) {
        let same = self
            .live_window
            .as_ref()
            .is_some_and(|session| session.id == id);
        self.end_live_window(ctx);
        if same {
            return;
        }
        if let Err(message) = self.begin_live_window(ctx, id) {
            self.canvas_notice = Some((message.to_owned(), Instant::now()));
        }
    }

    fn begin_live_window(&mut self, ctx: &egui::Context, id: PreviewId) -> Result<(), &str> {
        let main = self
            .main_hwnd
            .ok_or("Pluriview's window is not ready yet.")?;
        let preview = self
            .preview_manager
            .get(id)
            .filter(|preview| {
                preview.is_window_capture()
                    && preview.removing.is_none()
                    && !preview.manually_frozen
                    && !preview.left_click_disabled
            })
            .ok_or("This tile's window is not available right now.")?;
        let hwnd = preview
            .window_handle
            .as_ref()
            .map(|handle| handle.hwnd)
            .ok_or("This tile's window is not available right now.")?;
        let (tile_width, crop, pinned) = (
            preview.size.x,
            preview.crop_uv,
            preview.viewport_pin.is_some(),
        );
        if !native::is_alive(hwnd) {
            return Err("This tile's window is not available right now.");
        }
        if native::is_cloaked(hwnd) {
            return Err("This window is on another desktop. Switch to it to use the app.");
        }
        if native::is_hung(hwnd) {
            return Err("This app is not responding.");
        }

        // The live WebView is a native window above the canvas; park it.
        if let Some(active) = self.browser.active_id() {
            self.browser.park_all();
            self.capture_coordinator.pause_capture(active);
            self.browser_activated_at = None;
        }

        let was_below = native::is_below(hwnd, main);
        let minimized_from = native::is_minimized(hwnd)
            .then(|| native::placement(hwnd))
            .flatten();
        // Pluriview's own place is restored separately, via `was_below`.
        let covered_by = if minimized_from.is_none() {
            native::windows_above(hwnd, &[std::process::id()])
        } else {
            Vec::new()
        };
        if minimized_from.is_some() {
            native::show_without_activating(hwnd);
        }
        let frame = native::read_frame(hwnd).ok_or("Could not read the window's position.")?;
        // Moving the window to Pluriview's monitor rescales it by the two
        // monitors' ratio, so frame the tile for its size after the move.
        let canvas_dpi = native::monitor_dpi(main);
        let visible_size = native::size_after_dpi_change(
            frame.visible.size(),
            native::monitor_dpi(hwnd),
            canvas_dpi,
        );
        if !pinned {
            let zoom =
                native::native_zoom(tile_width, crop, visible_size.0, ctx.pixels_per_point());
            self.canvas.frame_live_window(
                id,
                zoom,
                screen_in_points(main, ctx.pixels_per_point()),
                &self.preview_manager,
                ctx.input(|i| i.time),
            );
        }
        // Resize handles would poke out around the window.
        self.canvas.selection.clear();
        if self.window_controls.always_on_top {
            // Drop it now rather than after this frame, before the window is
            // brought in front.
            native::drop_topmost(main);
            self.window_controls.topmost_suspended = true;
        }
        self.live_window = Some(LiveWindowSession {
            id,
            hwnd,
            pid: native::process_id(hwnd),
            home: frame.outer.origin(),
            known: frame.outer.origin(),
            minimized_from,
            was_below,
            covered_by,
            visible_size,
            dpi: canvas_dpi,
            placed_zoom: None,
            refits: 0,
            phase: Phase::Framing,
            moved_at: None,
            placed_at: None,
            recorded: None,
        });
        self.canvas.live_window.id = Some(id);
        ctx.request_repaint();
        Ok(())
    }

    /// Put the window in use back and return the camera.
    pub(super) fn end_live_window(&mut self, ctx: &egui::Context) {
        if let Some(session) = self.live_window.take() {
            let framing = session.phase == Phase::Framing;
            self.capture_in_use(&session, false);
            self.release_live_window(session, Ending::Restore);
            self.canvas
                .restore_live_window_camera(ctx.input(|i| i.time), framing);
            ctx.request_repaint();
        }
    }

    /// The tile's capture shows the pointer and the app's menus, smoothly,
    /// only while the app is used from it.
    fn capture_in_use(&mut self, session: &LiveWindowSession, in_use: bool) {
        let id = session.id;
        // A closed window's capture is ending anyway; restarting it would
        // only fail.
        if !native::exists(session.hwnd) {
            return;
        }
        self.capture_coordinator.set_in_use(id, in_use);
        if let Some(preview) = self.preview_manager.get(id) {
            let fps = if in_use {
                preview.target_fps.max(IN_USE_FPS.as_u32())
            } else {
                preview.target_fps
            };
            self.capture_coordinator.set_target_fps(id, fps);
        }
    }

    /// Put the window back without touching the camera, for workspace
    /// changes and exit.
    pub(super) fn drop_live_window(&mut self) {
        if let Some(session) = self.live_window.take() {
            self.release_live_window(session, Ending::Restore);
        }
    }

    /// "Show Original Window": the app's own window, in front, where it is.
    pub(super) fn show_original_window(&mut self, ctx: &egui::Context, id: PreviewId) {
        if self
            .live_window
            .as_ref()
            .is_some_and(|session| session.id == id)
        {
            self.end_live_window(ctx);
        }
        let Some(hwnd) = self
            .preview_manager
            .get(id)
            .filter(|preview| preview.is_window_capture())
            .and_then(|preview| preview.window_handle.as_ref())
            .map(|handle| handle.hwnd)
        else {
            return;
        };
        // Only a minimized window is restored; restoring a maximized one
        // would shrink it.
        if native::is_minimized(hwnd) {
            unsafe {
                let _ = ShowWindow(HWND(hwnd as *mut _), SW_RESTORE);
            }
        }
        native::raise_and_focus(hwnd);
    }

    /// Per-frame upkeep after the canvas pass: frame, place, follow and end
    /// the window in use.
    pub(super) fn live_window_frame(&mut self, ctx: &egui::Context, exit_pressed: bool) {
        if let Some(mut session) = self.live_window.take() {
            match self.step_live_window(ctx, &mut session, exit_pressed) {
                None => {
                    self.record_window_in_use(&mut session);
                    self.live_window = Some(session);
                }
                Some(ending) => {
                    let framing = session.phase == Phase::Framing;
                    self.capture_in_use(&session, false);
                    self.release_live_window(session, ending);
                    self.canvas
                        .restore_live_window_camera(ctx.input(|i| i.time), framing);
                    ctx.request_repaint();
                }
            }
        }
        if self.live_window.is_none() {
            self.canvas.live_window.id = None;
            self.canvas.live_window.window_rect = None;
        }
    }

    fn step_live_window(
        &mut self,
        ctx: &egui::Context,
        session: &mut LiveWindowSession,
        exit_pressed: bool,
    ) -> Option<Ending> {
        let Some(main) = self.main_hwnd else {
            return Some(Ending::Restore);
        };
        if !native::is_alive(session.hwnd) {
            return Some(Ending::Gone);
        }
        let id = session.id;
        let usable = self.preview_manager.get(id).is_some_and(|preview| {
            preview.is_window_capture()
                && preview.removing.is_none()
                && !preview.manually_frozen
                && !preview.hidden
                && !preview.left_click_disabled
                && preview.picture_in_picture.is_none()
                && preview
                    .window_handle
                    .as_ref()
                    .is_some_and(|handle| handle.hwnd == session.hwnd)
        });
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        if exit_pressed
            || self.canvas.live_window.done
            || !usable
            || minimized
            || self.window_controls.click_through
            || self.canvas.is_editing_crop(id)
            || self.browser.active_id().is_some()
            || native::is_minimized(session.hwnd)
        {
            return Some(Ending::Restore);
        }
        let Some(frame) = native::read_frame(session.hwnd) else {
            return Some(Ending::Gone);
        };
        let pixels_per_point = ctx.pixels_per_point();
        let client = native::client_origin(main);
        let (tile_width, crop, pinned) = self
            .preview_manager
            .get(id)
            .map(|preview| {
                (
                    preview.size.x,
                    preview.crop_uv,
                    preview.viewport_pin.is_some(),
                )
            })
            .unwrap_or_default();
        let canvas_rect = self.canvas.last_screen_rect?;

        // Right after Pluriview moves the window, or when it reaches another
        // monitor, a size change is the app adapting to that monitor's scale.
        // Any other is the user resizing, maximizing or snapping it.
        if matches!(session.phase, Phase::Live { .. })
            && frame.visible.size() != session.visible_size
        {
            let dpi = native::monitor_dpi(session.hwnd);
            let rescaled = dpi != session.dpi
                || session
                    .placed_at
                    .is_some_and(|at| at.elapsed() < SETTLE_TIME);
            if !rescaled || session.refits >= MAX_REFITS || native::is_maximized(session.hwnd) {
                return Some(Ending::Leave);
            }
            session.refits += 1;
            session.dpi = dpi;
            session.visible_size = frame.visible.size();
            // Where the app put itself while rescaling is not a drag by the
            // user; the tile stays and the window is placed over it again.
            session.known = frame.outer.origin();
            if !pinned {
                let zoom =
                    native::native_zoom(tile_width, crop, session.visible_size.0, pixels_per_point);
                if self.canvas.refit_live_window(
                    id,
                    zoom,
                    screen_in_points(main, pixels_per_point),
                    &self.preview_manager,
                ) {
                    session.placed_zoom = Some(self.canvas.zoom);
                }
            }
        }

        let tile = self.canvas.tile_screen_rect(id, &self.preview_manager)?;
        if !tile.intersects(canvas_rect) {
            return Some(Ending::Restore);
        }
        let target = |tile: egui::Rect| {
            frame.outer_origin_for(native::visible_origin_for_tile(
                tile,
                crop,
                session.visible_size,
                pixels_per_point,
                client,
            ))
        };
        let now = Instant::now();
        let mut poll = IDLE_POLL;

        match session.phase {
            Phase::Framing => {
                if self.canvas.is_camera_transitioning() {
                    ctx.request_repaint();
                    self.show_live_window_bar(session, None, tile, crop, pixels_per_point, client);
                    return None;
                }
                self.canvas.settle_live_window_camera();
                // Only the foreground app may hand the keyboard to another.
                if native::foreground() != main {
                    return Some(Ending::Restore);
                }
                let tile = self.canvas.tile_screen_rect(id, &self.preview_manager)?;
                let goal = target(tile);
                if !native::move_window(session.hwnd, goal) {
                    self.canvas_notice = Some((
                        "Pluriview cannot move this window. Apps running as administrator can only be used in their own window."
                            .to_owned(),
                        now,
                    ));
                    return Some(Ending::Restore);
                }
                session.placed_zoom = (!pinned).then_some(self.canvas.zoom);
                session.phase = Phase::Moving {
                    target: goal,
                    size: frame.visible.size(),
                    since: now,
                    raise: true,
                };
                poll = ACTIVE_POLL;
            }
            Phase::Moving {
                target: goal,
                size,
                since,
                raise,
            } => {
                if frame.outer.origin() == goal
                    || frame.visible.size() != size
                    || since.elapsed() >= MOVE_TIMEOUT
                {
                    session.known = frame.outer.origin();
                    session.placed_at = Some(now);
                    if raise {
                        native::raise_and_focus(session.hwnd);
                    }
                    // Only now: before the window is over its tile, a pointer
                    // over its old spot would show on the tile somewhere else.
                    self.capture_in_use(session, true);
                    session.phase = Phase::Live { since: now };
                }
                poll = ACTIVE_POLL;
            }
            Phase::Live { since } => {
                // Zooming the canvas changes the tile's scale; the window
                // cannot follow that.
                if session
                    .placed_zoom
                    .is_some_and(|zoom| (self.canvas.zoom / zoom - 1.0).abs() > 0.005)
                {
                    return Some(Ending::Restore);
                }
                let current = frame.outer.origin();
                if self.canvas.live_window.released {
                    // Back from the grab bar: over the tile's new place, in front.
                    let goal = target(tile);
                    if goal != current {
                        native::move_window(session.hwnd, goal);
                        session.phase = Phase::Moving {
                            target: goal,
                            size: frame.visible.size(),
                            since: now,
                            raise: true,
                        };
                    } else {
                        native::raise_and_focus(session.hwnd);
                        session.phase = Phase::Live { since: now };
                    }
                    poll = ACTIVE_POLL;
                } else if self.canvas.live_window.held
                    || ctx
                        .input(|i| i.pointer.hover_pos())
                        .is_some_and(|pointer| self.canvas.live_window.covers(pointer))
                {
                    // The grab bar has the pointer; the window waits behind
                    // Pluriview until it is let go. Pointing at the bar counts
                    // too, so the click that brings Pluriview forward is never
                    // read as leaving the app.
                    poll = ACTIVE_POLL;
                } else {
                    let foreground = native::foreground();
                    if foreground != 0
                        && !native::foreground_belongs_to(foreground, session.hwnd, session.pid)
                        && since.elapsed() >= FOCUS_GRACE
                    {
                        return Some(Ending::Restore);
                    }
                    if current != session.known {
                        // The user dragged the window itself: the tile follows.
                        let delta = egui::vec2(
                            (current.0 - session.known.0) as f32,
                            (current.1 - session.known.1) as f32,
                        ) / pixels_per_point;
                        self.canvas
                            .follow_live_window(id, delta, &mut self.preview_manager);
                        session.known = current;
                        session.moved_at = Some(now);
                    } else {
                        // Keep the window on its tile when the canvas shifts.
                        let goal = target(tile);
                        if (goal.0 - current.0).abs() > 1 || (goal.1 - current.1).abs() > 1 {
                            if !native::move_window(session.hwnd, goal) {
                                return Some(Ending::Restore);
                            }
                            session.phase = Phase::Moving {
                                target: goal,
                                size: frame.visible.size(),
                                since: now,
                                raise: false,
                            };
                        }
                    }
                    if session
                        .moved_at
                        .is_some_and(|at| at.elapsed() < ACTIVE_AFTER_MOVE)
                        || matches!(session.phase, Phase::Moving { .. })
                    {
                        poll = ACTIVE_POLL;
                    }
                }
            }
        }

        let tile = self.canvas.tile_screen_rect(id, &self.preview_manager)?;
        let shown = matches!(session.phase, Phase::Live { .. }).then_some(frame);
        self.show_live_window_bar(session, shown, tile, crop, pixels_per_point, client);
        ctx.request_repaint_after(poll);
        None
    }

    /// Tell the canvas where the window covers it, for the grab bar: where it
    /// actually is once over the tile, else where it is going.
    fn show_live_window_bar(
        &mut self,
        session: &LiveWindowSession,
        shown: Option<native::WindowFrame>,
        tile: egui::Rect,
        crop: Option<(f32, f32, f32, f32)>,
        pixels_per_point: f32,
        client: (i32, i32),
    ) {
        let (origin, size) = match shown {
            Some(frame) => (frame.visible.origin(), frame.visible.size()),
            None => (
                native::visible_origin_for_tile(
                    tile,
                    crop,
                    session.visible_size,
                    pixels_per_point,
                    client,
                ),
                session.visible_size,
            ),
        };
        self.canvas.live_window.id = Some(session.id);
        self.canvas.live_window.window_rect = Some(native::visible_rect_in_points(
            origin,
            size,
            pixels_per_point,
            client,
        ));
    }

    /// Hand the window back. Restoring moves it home (or minimizes it again),
    /// behind the windows that covered it, and puts Pluriview back over it if
    /// it covered the window before.
    fn release_live_window(&mut self, session: LiveWindowSession, ending: Ending) {
        if ending == Ending::Restore && !native::is_hung(session.hwnd) {
            if session.phase == Phase::Framing {
                // Never moved; only a window shown for this goes back down.
                if let Some(placement) = session.minimized_from {
                    native::minimize_into(session.hwnd, placement);
                }
            } else if native::is_minimized(session.hwnd) {
                // Minimized over its tile: restoring it later opens it home.
                native::shift_restored_position(
                    session.hwnd,
                    (
                        session.home.0 - session.known.0,
                        session.home.1 - session.known.1,
                    ),
                );
            } else if let Some(placement) = session.minimized_from {
                native::minimize_into(session.hwnd, placement);
            } else {
                native::move_window(session.hwnd, session.home);
                // Using it brought it to the front; it goes back behind the
                // windows that covered it.
                native::place_below(session.hwnd, &session.covered_by);
            }
            // Only when the window is still in front: Pluriview may already
            // be, and must not drop behind windows between the two.
            if let Some(main) = self
                .main_hwnd
                .filter(|main| session.was_below && !native::is_below(session.hwnd, *main))
            {
                native::place_above(main, session.hwnd);
            }
        }
        if session.recorded.is_some() {
            if let Some(storage) = &self.storage {
                if let Err(error) = std::fs::remove_file(storage.window_in_use_path()) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        log::warn!("Could not remove the window-in-use record: {error}");
                    }
                }
            }
        }
        // Placing Pluriview above the window cleared its always-on-top
        // level; the next frame restores it.
        self.window_controls.topmost_suspended = false;
        self.canvas.live_window.id = None;
        self.canvas.live_window.window_rect = None;
    }

    /// Keep the crash-recovery file in step with where Pluriview left the
    /// window, at most twice a second while it moves.
    fn record_window_in_use(&self, session: &mut LiveWindowSession) {
        let placed = match session.phase {
            // Not moved yet.
            Phase::Framing => return,
            Phase::Moving { target, .. } => target,
            Phase::Live { .. } => session.known,
        };
        if session.recorded.is_some_and(|(recorded, at)| {
            recorded == placed || at.elapsed() < RECOVERY_SAVE_INTERVAL
        }) {
            return;
        }
        let Some(storage) = &self.storage else {
            return;
        };
        let record = WindowInUse {
            hwnd: session.hwnd,
            pid: session.pid,
            home: session.home,
            placed,
            minimized: session.minimized_from.is_some(),
        };
        // Also after a failure, so a failing disk is not retried every frame.
        session.recorded = Some((placed, Instant::now()));
        if let Err(error) = write_window_in_use(&storage.window_in_use_path(), &record) {
            log::warn!("Could not record the window in use: {error}");
        }
    }
}

fn write_window_in_use(path: &std::path::Path, record: &WindowInUse) -> std::io::Result<()> {
    let json = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    // Swap a complete file in, so a crash mid-write keeps the previous one.
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, json)?;
    std::fs::rename(&temporary, path)
}

/// On launch: if Pluriview crashed while an app was used from its tile, put
/// that window back where it came from. Only when it is still exactly where
/// Pluriview left it; a window the user has moved, resized to fill the
/// screen, minimized or closed since stays as it is.
pub(super) fn recover_window_in_use(path: &std::path::Path) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let _ = std::fs::remove_file(path);
    let Ok(record) = serde_json::from_slice::<WindowInUse>(&bytes) else {
        return;
    };
    let untouched = native::is_alive(record.hwnd)
        && native::process_id(record.hwnd) == record.pid
        && !native::is_minimized(record.hwnd)
        && !native::is_maximized(record.hwnd)
        && !native::is_hung(record.hwnd)
        && native::read_frame(record.hwnd)
            .is_some_and(|frame| frame.outer.origin() == record.placed);
    if !untouched {
        return;
    }
    if record.minimized {
        native::shift_restored_position(
            record.hwnd,
            (
                record.home.0 - record.placed.0,
                record.home.1 - record.placed.1,
            ),
        );
    } else {
        native::move_window(record.hwnd, record.home);
    }
}

/// The usable part (without the taskbar) of the monitor Pluriview is on, in
/// canvas points: an app window used in place is kept on it.
fn screen_in_points(main: isize, pixels_per_point: f32) -> Option<egui::Rect> {
    let work = native::work_area(main)?;
    Some(native::visible_rect_in_points(
        work.origin(),
        work.size(),
        pixels_per_point,
        native::client_origin(main),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_interaction::test_windows::{pump_messages, TestWindow};

    fn arrives(handle: isize, origin: (i32, i32)) -> bool {
        for _ in 0..100 {
            pump_messages();
            if native::read_frame(handle).is_some_and(|frame| frame.outer.origin() == origin) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    #[ignore = "moves native windows; run with --ignored on a Windows desktop"]
    fn a_window_left_in_use_by_a_crash_goes_back_on_the_next_launch() {
        let directory = std::env::temp_dir().join(format!(
            "pluriview-window-in-use-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("window_in_use.json");

        let window = TestWindow::new(None);
        pump_messages();
        let home = native::read_frame(window.handle()).unwrap().outer.origin();
        let placed = (home.0 + 700, home.1 + 300);
        assert!(native::move_window(window.handle(), placed));
        assert!(arrives(window.handle(), placed));
        let record = |home, placed| WindowInUse {
            hwnd: window.handle(),
            pid: std::process::id(),
            home,
            placed,
            minimized: false,
        };

        // Moved by the user since: left where it is, and the record used up.
        write_window_in_use(&path, &record(home, (placed.0 + 1, placed.1))).unwrap();
        recover_window_in_use(&path);
        assert!(!path.exists());
        assert!(!arrives(window.handle(), home));

        // A reused handle in another process is never touched.
        let other = WindowInUse {
            pid: std::process::id() + 1,
            ..record(home, placed)
        };
        write_window_in_use(&path, &other).unwrap();
        recover_window_in_use(&path);
        assert!(!arrives(window.handle(), home));

        // Still where Pluriview left it: back home.
        write_window_in_use(&path, &record(home, placed)).unwrap();
        recover_window_in_use(&path);
        assert!(!path.exists());
        assert!(arrives(window.handle(), home));

        // One that was minimized goes back minimized, to open at home.
        assert!(native::move_window(window.handle(), placed));
        assert!(arrives(window.handle(), placed));
        let minimized = WindowInUse {
            minimized: true,
            ..record(home, placed)
        };
        write_window_in_use(&path, &minimized).unwrap();
        recover_window_in_use(&path);
        pump_messages();
        assert!(native::is_minimized(window.handle()));
        native::show_without_activating(window.handle());
        assert!(arrives(window.handle(), home));

        let _ = std::fs::remove_dir_all(&directory);
    }
}
