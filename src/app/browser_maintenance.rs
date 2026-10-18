//! Keeping browser tiles healthy: applying WebView2 engine updates without
//! restarting Pluriview, recovering from a lost browser process, and clearing
//! the shared browser profile.

use super::*;
use crate::browser::PageCrash;

/// A browser process that exits again this soon after a recovery is not
/// restarted automatically, so a broken runtime cannot restart in a loop.
const ENGINE_RECOVERY_COOLDOWN: Duration = Duration::from_secs(60);
/// How often to check whether the old browser process has exited.
const ENGINE_RESTART_POLL: Duration = Duration::from_millis(100);

/// Runtime-only browser upkeep state.
#[derive(Default)]
pub(super) struct BrowserMaintenance {
    /// The Clear Browser Data confirmation is open.
    pub(super) confirm_clear: bool,
    /// The user was told about the engine update currently waiting.
    update_announced: bool,
    /// When tiles were last restarted after the browser process exited.
    last_recovery: Option<Instant>,
}

impl PluriviewApp {
    /// Handle engine updates, a lost browser process and finished data
    /// clearing. Returns true while restarted tiles must wait for the old
    /// engine to exit. Call once per frame, before starting pending tiles.
    pub(super) fn browser_maintenance_upkeep(&mut self, ctx: &egui::Context) -> bool {
        self.canvas.show_sound_badges = self.app_config.show_sound_badges;
        if self.browser.take_engine_lost() {
            self.recover_lost_browser_engine();
        }

        if self.browser.engine_update_ready() {
            if !self.browser_maintenance.update_announced {
                self.browser_maintenance.update_announced = true;
                self.canvas_notice = Some((
                    "A browser security update is ready. Choose Tools → Restart Browser Tiles to apply it."
                        .to_owned(),
                    Instant::now(),
                ));
            }
        } else {
            self.browser_maintenance.update_announced = false;
        }

        if let Some(result) = self.browser.poll_clear_browsing_data() {
            let message = match result {
                Ok(()) => "Browser data cleared. Sites in browser tiles are signed out.".to_owned(),
                Err(error) => {
                    log::error!("Could not clear browser data: {error}");
                    "Browser data could not be cleared.".to_owned()
                }
            };
            self.canvas_notice = Some((message, Instant::now()));
        } else if self.browser.clearing_browsing_data() {
            ctx.request_repaint_after(ENGINE_RESTART_POLL);
        }

        let restarting = self.browser.engine_restarting();
        if restarting {
            ctx.request_repaint_after(ENGINE_RESTART_POLL);
        }
        restarting
    }

    /// Close every browser tile's page and start it again on a fresh engine,
    /// keeping its address, mute state, geometry and last frame. Tiles left
    /// without an engine by an earlier failed recovery come back as well.
    pub(super) fn restart_browser_tiles(&mut self) {
        let closed: HashMap<PreviewId, String> =
            self.browser.shut_down_engine().into_iter().collect();
        self.browser_activated_at = None;
        let stranded: Vec<PreviewId> = self
            .preview_manager
            .all()
            .filter(|preview| {
                preview.is_browser()
                    && !closed.contains_key(&preview.id)
                    && !self.pending_browser_tiles.contains_key(&preview.id)
            })
            .map(|preview| preview.id)
            .collect();
        for id in closed.keys().copied().chain(stranded) {
            self.capture_coordinator.stop_capture(id);
            let Some(preview) = self.preview_manager.get_mut(id) else {
                continue;
            };
            let Some(url) = closed
                .get(&id)
                .cloned()
                .or_else(|| preview.browser_url.clone())
            else {
                continue;
            };
            preview.window_handle = None;
            preview.discard_pending_capture_frame();
            preview.clear_capture_error();
            preview.browser_status = BrowserTileStatus::Starting;
            preview.browser_waiting_for_content = true;
            preview.browser_playing_audio = false;
            self.pending_browser_tiles.insert(
                id,
                PendingBrowserTile {
                    url,
                    fps: preview.fps_preset,
                    muted: preview.browser_muted,
                    shown_once: true,
                    restore_deferred: false,
                },
            );
        }
    }

    fn recover_lost_browser_engine(&mut self) {
        let repeated = self
            .browser_maintenance
            .last_recovery
            .is_some_and(|at| at.elapsed() < ENGINE_RECOVERY_COOLDOWN);
        if !repeated {
            log::warn!("The WebView2 browser process exited; restarting browser tiles");
            self.browser_maintenance.last_recovery = Some(Instant::now());
            self.restart_browser_tiles();
            self.canvas_notice = Some((
                "Browser tiles restarted because the browser engine stopped.".to_owned(),
                Instant::now(),
            ));
            return;
        }

        log::error!("The WebView2 browser process exited again; leaving browser tiles stopped");
        let closed = self.browser.shut_down_engine();
        self.browser_activated_at = None;
        for (id, url) in closed {
            self.capture_coordinator.stop_capture(id);
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.window_handle = None;
                preview.browser_url = Some(url);
                preview.set_capture_error(
                    "The browser engine keeps stopping. Choose Tools → Restart Browser Tiles to try again."
                        .to_owned(),
                );
            }
        }
    }

    /// Tell the user about a page that crashed and what the tile did.
    pub(super) fn note_browser_page_crash(&mut self, crash: PageCrash) {
        let message = match crash {
            PageCrash::Reloaded => "A browser tile's page crashed and was reloaded.",
            PageCrash::Repeated => {
                "A browser tile's page keeps crashing. Reload it from the tile menu to try again."
            }
        };
        self.canvas_notice = Some((message.to_owned(), Instant::now()));
    }

    /// Restart and Clear Browser Data entries for the Tools menu.
    pub(super) fn browser_maintenance_menu(&mut self, ui: &mut egui::Ui) {
        let has_browser_tiles = self.preview_manager.all().any(Preview::is_browser);
        let restart_label = if self.browser.engine_update_ready() {
            "Restart Browser Tiles (update ready)"
        } else {
            "Restart Browser Tiles"
        };
        if ui
            .add_enabled(has_browser_tiles, egui::Button::new(restart_label))
            .on_hover_text("Reload every browser tile on a fresh browser engine. This applies WebView2 security updates without restarting Pluriview.")
            .clicked()
        {
            self.restart_browser_tiles();
            ui.close_menu();
        }

        let can_clear =
            self.browser.ids().next().is_some() && !self.browser.clearing_browsing_data();
        if ui
            .add_enabled(can_clear, egui::Button::new("Clear Browser Data..."))
            .on_hover_text("Sign out of every site and erase cookies, site data, history and cache for all browser tiles.")
            .on_disabled_hover_text("Open a browser tile to clear the data browser tiles share.")
            .clicked()
        {
            self.browser_maintenance.confirm_clear = true;
            ui.close_menu();
        }
    }

    pub(super) fn browser_data_confirmation_ui(&mut self, ctx: &egui::Context) {
        if !self.browser_maintenance.confirm_clear {
            return;
        }
        let mut clear = false;
        let mut cancel = false;
        egui::Window::new("Clear Browser Data")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Sign out of every site in browser tiles?");
                ui.label(
                    egui::RichText::new(
                        "Cookies, logins, site data, history and cache are erased for all browser tiles. uBlock Origin Lite stays installed.",
                    )
                    .small()
                    .weak(),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Clear Data").clicked() {
                        clear = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if clear {
            self.browser_maintenance.confirm_clear = false;
            if let Err(error) = self.browser.clear_browsing_data() {
                log::error!("Could not clear browser data: {error}");
                self.canvas_notice = Some((error, Instant::now()));
            }
        } else if cancel {
            self.browser_maintenance.confirm_clear = false;
        }
    }
}
