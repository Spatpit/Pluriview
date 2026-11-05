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
                self.hotkey_conflict = None;
            } else {
                // A quick tap can begin and end between polls; Windows retains
                // the pressed edge, so it is still a valid single-key binding.
                self.assign_recorded_hotkey(slot, Hotkey::key(first_key));
            }
        }
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
        // Leave room for the tabs and the Close row on short screens.
        let scroll_height = (ctx.content_rect().height() - 260.0).clamp(220.0, 560.0);
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(620.0)
            .min_width(520.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(&ctx.global_style()).fill(egui::Color32::from_rgb(25, 25, 28)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for (tab, icon, label) in [
                        (
                            SettingsTab::Shortcuts,
                            egui_phosphor::regular::KEYBOARD,
                            "Keyboard Shortcuts",
                        ),
                        (
                            SettingsTab::ExternalTools,
                            egui_phosphor::regular::WRENCH,
                            "External Tools",
                        ),
                    ] {
                        let selected = self.settings_tab == tab;
                        let text = egui::RichText::new(format!("{icon}  {label}")).size(14.5);
                        let text = if selected {
                            text.color(crate::ui_theme::GOLD)
                        } else {
                            text
                        };
                        if ui.selectable_label(selected, text).clicked() && !selected {
                            self.settings_tab = tab;
                            self.stop_hotkey_recording();
                        }
                    }
                });
                ui.separator();
                ui.add_space(4.0);

                match self.settings_tab {
                    SettingsTab::Shortcuts => self.shortcuts_tab(ui, scroll_height),
                    SettingsTab::ExternalTools => {
                        egui::ScrollArea::vertical()
                            .id_salt("settings_external_tools_scroll")
                            .max_height(scroll_height)
                            .show(ui, |ui| {
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
                                if ui.button("Re-scan All").clicked() {
                                    self.external_tools.rescan_all();
                                }
                            });
                    }
                }

                if let Some(error) = &self.config_error {
                    ui.add_space(6.0);
                    ui.colored_label(egui::Color32::from_rgb(235, 120, 120), error);
                }
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        self.show_settings = open && !close;
        if !self.show_settings {
            self.stop_hotkey_recording();
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
            self.settings_tab = SettingsTab::ExternalTools;
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
