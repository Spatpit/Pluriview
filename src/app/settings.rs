use super::*;

impl PluriviewApp {
    pub(super) fn hotkey_capture_upkeep(&mut self, ctx: &egui::Context) {
        let Some(slot) = self.hotkey_recording else {
            return;
        };
        ctx.request_repaint_after(Duration::from_millis(30));

        #[cfg(windows)]
        if !self.owns_foreground() {
            return;
        }

        if let Some(first_key) = self.hotkey_recording_first {
            if let Some(second_key) = self
                .hotkey_tracker
                .newly_pressed_key_except(Some(first_key))
            {
                self.assign_recorded_hotkey(slot, Hotkey::pair(first_key, second_key));
            } else if !self.hotkey_tracker.is_down(first_key) {
                self.assign_recorded_hotkey(slot, Hotkey::key(first_key));
            }
        } else if let Some(first_key) = self.hotkey_tracker.newly_pressed_key_except(None) {
            let simultaneous_second = self
                .hotkey_tracker
                .newly_pressed_key_except(Some(first_key))
                .filter(|second| self.hotkey_tracker.is_down(*second));
            if let Some(second_key) = simultaneous_second {
                self.assign_recorded_hotkey(slot, Hotkey::pair(first_key, second_key));
            } else if self.hotkey_tracker.is_down(first_key) {
                self.hotkey_recording_first = Some(first_key);
                self.hotkey_error = None;
            } else {
                // A quick tap can begin and end between polls; Windows retains
                // the pressed edge, so it is still a valid single-key binding.
                self.assign_recorded_hotkey(slot, Hotkey::key(first_key));
            }
        }
    }

    pub(super) fn assign_recorded_hotkey(&mut self, slot: HotkeySlot, hotkey: Hotkey) {
        if let Some(conflict) = self.app_config.keyboard_shortcuts.conflict(slot, hotkey) {
            self.hotkey_error = Some(format!(
                "{} is already assigned to {}.",
                hotkey.display(),
                conflict.label()
            ));
            self.hotkey_recording_first = None;
        } else {
            self.app_config.keyboard_shortcuts.set(slot, hotkey);
            self.hotkey_recording = None;
            self.hotkey_recording_first = None;
            self.hotkey_error = None;
            self.save_app_config();
        }
    }

    pub(super) fn shortcut_settings_row(&mut self, ui: &mut egui::Ui, slot: HotkeySlot) {
        ui.label(slot.label());
        let recording = self.hotkey_recording == Some(slot);
        let text = if recording {
            self.hotkey_recording_first.map_or_else(
                || "Press a key…".to_owned(),
                |first| format!("{} + …", Hotkey::key(first).display()),
            )
        } else {
            self.app_config.keyboard_shortcuts.get(slot).display()
        };
        let button = egui::Button::new(text).min_size(Vec2::new(180.0, 24.0));
        if ui.add(button).clicked() {
            self.hotkey_recording = Some(slot);
            self.hotkey_recording_first = None;
            self.hotkey_error = None;
        }
        ui.end_row();
    }

    pub(super) fn settings_ui(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            self.hotkey_recording = None;
            self.hotkey_recording_first = None;
            return;
        }

        self.hotkey_capture_upkeep(ctx);

        let mut open = self.show_settings;
        let mut close = false;
        let mut action = None;
        let mut restore_hotkeys = false;
        let is_viewpoint_shortcut = |slot: HotkeySlot| {
            matches!(
                slot,
                HotkeySlot::PreviousCanvasView | HotkeySlot::NextCanvasView
            ) || HotkeySlot::CANVAS_VIEWS.contains(&slot)
        };
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(600.0)
            .default_height(680.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgb(25, 25, 28)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(580.0)
                    .show(ui, |ui| {
                        ui.heading("External tools");
                        ui.label(
                            egui::RichText::new(
                                "Video files play with bundled libmpv. Streamlink is only needed for live stream URLs.",
                            )
                            .weak(),
                        );
                        ui.add_space(8.0);
                        for kind in ToolKind::SETTINGS {
                            ui.group(|ui| {
                                Self::external_tool_settings_row(
                                    ui,
                                    kind,
                                    &self.external_tools,
                                    &mut action,
                                );
                            });
                            ui.add_space(8.0);
                        }
                        ui.separator();
                        ui.add_space(12.0);
                        ui.heading("Keyboard shortcuts");
                        ui.label(
                            egui::RichText::new(
                                "Click a shortcut, then tap one key, or hold one key and press a second. Mouse controls are not changed here.",
                            )
                            .weak(),
                        );
                        ui.add_space(8.0);

                        egui::Grid::new("settings_hotkeys_grid")
                            .num_columns(2)
                            .spacing([24.0, 7.0])
                            .show(ui, |ui| {
                                for slot in HotkeySlot::ALL.into_iter().filter(|slot| !is_viewpoint_shortcut(*slot)) {
                                    self.shortcut_settings_row(ui, slot);
                                }
                            });

                        ui.add_space(12.0);
                        let viewpoints = egui::CollapsingHeader::new("Saved Viewpoints")
                            .id_salt("settings_saved_viewpoint_shortcuts")
                            .default_open(false)
                            .show(ui, |ui| {
                                egui::Grid::new("settings_viewpoint_hotkeys_grid")
                                    .num_columns(2)
                                    .spacing([24.0, 7.0])
                                    .show(ui, |ui| {
                                        for slot in HotkeySlot::ALL.into_iter().filter(|slot| is_viewpoint_shortcut(*slot)) {
                                            self.shortcut_settings_row(ui, slot);
                                        }
                                    });
                            });
                        if viewpoints.body_response.is_none()
                            && self.hotkey_recording.is_some_and(is_viewpoint_shortcut)
                        {
                            self.hotkey_recording = None;
                            self.hotkey_recording_first = None;
                            self.hotkey_error = None;
                        }

                        if let Some(error) = &self.hotkey_error {
                            ui.add_space(6.0);
                            ui.colored_label(egui::Color32::from_rgb(235, 120, 120), error);
                        } else if self.hotkey_recording.is_some() {
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(
                                    "Listening… release for one key, or hold it and press another for a two-key shortcut.",
                                )
                                .weak(),
                            );
                        }

                        ui.add_space(8.0);
                        if ui.button("Restore Default Keys").clicked() {
                            restore_hotkeys = true;
                        }

                        if let Some(error) = &self.config_error {
                            ui.colored_label(egui::Color32::from_rgb(235, 120, 120), error);
                            ui.add_space(6.0);
                        }
                    });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Re-scan All").clicked() {
                        self.external_tools.rescan_all();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                });
            });
        self.show_settings = open && !close;
        if !self.show_settings {
            self.hotkey_recording = None;
            self.hotkey_recording_first = None;
            self.hotkey_error = None;
        }

        if restore_hotkeys {
            self.app_config.keyboard_shortcuts = HotkeyBindings::default();
            self.hotkey_recording = None;
            self.hotkey_recording_first = None;
            self.hotkey_error = None;
            self.save_app_config();
        }

        match action {
            Some(SettingsAction::Browse(kind)) => {
                if let Some(path) = external_tools::pick_executable(self.main_hwnd, kind) {
                    self.set_external_tool_override(kind, Some(path));
                }
            }
            Some(SettingsAction::UseAutoDetected(kind)) => {
                self.set_external_tool_override(kind, None);
            }
            Some(SettingsAction::Rescan(kind)) => self.external_tools.rescan(kind),
            None => {}
        }
    }

    pub(super) fn external_tool_error_ui(&mut self, ctx: &egui::Context) {
        let Some(message) = self.external_tool_error.clone() else {
            return;
        };
        let mut open_settings = false;
        let mut close = false;
        egui::Window::new("External Tools Required")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(message);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Open Settings").clicked() {
                        open_settings = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if open_settings {
            self.external_tool_error = None;
            self.show_settings = true;
        } else if close {
            self.external_tool_error = None;
        }
    }

    pub(super) fn video_action_error_ui(&mut self, ctx: &egui::Context) {
        let Some(message) = self.video_action_error.clone() else {
            return;
        };
        let mut dismiss = false;
        egui::Window::new("Video Control Error")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(message);
                ui.add_space(8.0);
                if ui.button("Dismiss").clicked() {
                    dismiss = true;
                }
            });
        if dismiss {
            self.video_action_error = None;
        }
    }

    pub(super) fn external_tool_settings_row(
        ui: &mut egui::Ui,
        kind: ToolKind,
        tools: &ExternalTools,
        action: &mut Option<SettingsAction>,
    ) {
        let status = tools.status(kind);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(kind.display_name()).strong().size(16.0));
            let (label, color) = match status {
                ToolStatus::Checking => ("Checking...", egui::Color32::from_rgb(140, 170, 235)),
                ToolStatus::Available { .. } => {
                    ("Available", egui::Color32::from_rgb(110, 205, 135))
                }
                ToolStatus::Invalid { .. } => ("Invalid", egui::Color32::from_rgb(235, 120, 120)),
                ToolStatus::Missing => ("Not found", egui::Color32::from_rgb(220, 180, 95)),
            };
            ui.colored_label(color, label);
        });

        let displayed_path = status.path().or_else(|| tools.override_path(kind));
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Path:").strong());
            ui.label(
                displayed_path
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "No executable detected".to_owned()),
            );
        });

        match status {
            ToolStatus::Available {
                version, source, ..
            } => {
                ui.label(format!("Version: {version}"));
                ui.label(egui::RichText::new(source.label()).small().weak());
            }
            ToolStatus::Invalid { error, source, .. } => {
                ui.colored_label(egui::Color32::from_rgb(235, 120, 120), error);
                ui.label(egui::RichText::new(source.label()).small().weak());
            }
            ToolStatus::Checking => {
                ui.label(
                    egui::RichText::new("Running --version off the UI thread")
                        .small()
                        .weak(),
                );
            }
            ToolStatus::Missing => {
                ui.label(
                    egui::RichText::new(
                        "Checked beside Pluriview, PATH, and common Windows install locations.",
                    )
                    .small()
                    .weak(),
                );
            }
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Browse...").clicked() {
                *action = Some(SettingsAction::Browse(kind));
            }
            if ui
                .add_enabled(
                    tools.override_path(kind).is_some(),
                    egui::Button::new("Use Auto-detected"),
                )
                .clicked()
            {
                *action = Some(SettingsAction::UseAutoDetected(kind));
            }
            if ui.button("Re-scan").clicked() {
                *action = Some(SettingsAction::Rescan(kind));
            }
        });
    }

    pub(super) fn set_external_tool_override(
        &mut self,
        kind: ToolKind,
        path: Option<std::path::PathBuf>,
    ) {
        match kind {
            ToolKind::Mpv => {}
            ToolKind::Streamlink => self.app_config.external_tools.streamlink_path = path.clone(),
        }
        self.external_tools.set_override(kind, path);
        self.save_app_config();
    }

    pub(super) fn save_app_config(&mut self) {
        self.config_error = match &self.storage {
            Some(storage) => storage
                .save_config(&self.app_config)
                .err()
                .map(|error| format!("Could not save settings: {error}")),
            None => Some("Settings storage is unavailable.".to_owned()),
        };
    }
}
