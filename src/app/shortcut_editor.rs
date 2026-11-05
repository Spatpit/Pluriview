//! The Keyboard Shortcuts tab of Settings: every shortcut in a labelled
//! group, a search box, and on each row its key to click and change, with
//! Cancel while listening, Reset once changed, and Swap on a conflict.
use super::*;
use egui_phosphor::regular as icons;

/// Which page of the Settings window is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SettingsTab {
    #[default]
    Shortcuts,
    ExternalTools,
}

/// A key that is already used by another shortcut, offered as a swap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HotkeyConflict {
    /// The shortcut being changed.
    pub slot: HotkeySlot,
    /// The key it was asked to take.
    pub hotkey: Hotkey,
    /// The shortcut that has that key now.
    pub other: HotkeySlot,
}

const VIEWPOINTS: &str = "Saved Viewpoints";

/// Every shortcut, once, in the groups the editor shows them in.
pub(super) const SHORTCUT_GROUPS: [(&str, &[HotkeySlot]); 6] = [
    (
        "Panels & View",
        &[
            HotkeySlot::ToggleWindowPicker,
            HotkeySlot::ToggleScreenshotsPanel,
            HotkeySlot::ToggleTileControl,
            HotkeySlot::ToggleGrid,
            HotkeySlot::ToggleCanvasOnly,
            HotkeySlot::ShowShortcutHelp,
        ],
    ),
    (
        "Tiles",
        &[
            HotkeySlot::SelectAll,
            HotkeySlot::DeleteSelected,
            HotkeySlot::UndoRemovedTiles,
            HotkeySlot::FocusCurrentTile,
            HotkeySlot::ExitTileOrBrowser,
            HotkeySlot::PictureInPicture,
        ],
    ),
    (
        "Browser Tiles",
        &[
            HotkeySlot::InteractBrowser,
            HotkeySlot::InteractBrowserAlternate,
        ],
    ),
    (
        "Groups & Layouts",
        &[
            HotkeySlot::GroupTiles,
            HotkeySlot::UngroupTiles,
            HotkeySlot::LayoutOneLarge,
            HotkeySlot::LayoutTwoLarge,
            HotkeySlot::LayoutThreeLarge,
            HotkeySlot::LayoutEqual,
            HotkeySlot::MakeMainTile,
            HotkeySlot::PreviousMainTile,
            HotkeySlot::NextMainTile,
            HotkeySlot::SizeMedium,
            HotkeySlot::SizeLarge,
        ],
    ),
    (
        "Screenshots & Recording",
        &[
            HotkeySlot::ScreenshotTile,
            HotkeySlot::RecordTile,
            HotkeySlot::RecordGif,
            HotkeySlot::PauseRecording,
        ],
    ),
    (
        VIEWPOINTS,
        &[
            HotkeySlot::PreviousCanvasView,
            HotkeySlot::NextCanvasView,
            HotkeySlot::CanvasView1,
            HotkeySlot::CanvasView2,
            HotkeySlot::CanvasView3,
            HotkeySlot::CanvasView4,
            HotkeySlot::CanvasView5,
            HotkeySlot::CanvasView6,
            HotkeySlot::CanvasView7,
            HotkeySlot::CanvasView8,
            HotkeySlot::CanvasView9,
        ],
    ),
];

/// Whether a shortcut matches every word typed in the search box, by its
/// name or its key ("ctrl g", "group", "layout equal").
pub(super) fn matches_search(slot: HotkeySlot, hotkey: Hotkey, query: &str) -> bool {
    let haystack = format!("{} {}", slot.label(), hotkey.display()).to_lowercase();
    query
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || c == '+')
        .filter(|word| !word.is_empty())
        .all(|word| haystack.contains(word))
}

/// Give the conflicting key to the shortcut being changed, and its old key
/// to the shortcut that had it, so every key stays used once.
fn swap(bindings: &mut HotkeyBindings, conflict: HotkeyConflict) {
    let previous = bindings.get(conflict.slot);
    bindings.set(conflict.other, previous);
    bindings.set(conflict.slot, conflict.hotkey);
}

/// What a shortcut row asked for this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowAction {
    Record,
    Cancel,
    Reset,
    Swap(HotkeyConflict),
}

const KEY_BUTTON_WIDTH: f32 = 150.0;
const ROW_ICON_WIDTH: f32 = 28.0;

impl PluriviewApp {
    pub(super) fn stop_hotkey_recording(&mut self) {
        self.hotkey_recording = None;
        self.hotkey_recording_first = None;
        self.hotkey_conflict = None;
    }

    /// Give `slot` the key `hotkey`, unless another shortcut has it: then
    /// keep listening and offer to swap.
    pub(super) fn assign_recorded_hotkey(&mut self, slot: HotkeySlot, hotkey: Hotkey) {
        if let Some(other) = self.app_config.keyboard_shortcuts.conflict(slot, hotkey) {
            self.hotkey_conflict = Some(HotkeyConflict {
                slot,
                hotkey,
                other,
            });
            self.hotkey_recording_first = None;
        } else {
            self.app_config.keyboard_shortcuts.set(slot, hotkey);
            self.stop_hotkey_recording();
            self.save_app_config();
        }
    }

    /// Give the conflicting key to the shortcut being changed, and its old
    /// key to the shortcut that had it.
    fn swap_hotkeys(&mut self, conflict: HotkeyConflict) {
        swap(&mut self.app_config.keyboard_shortcuts, conflict);
        self.stop_hotkey_recording();
        self.save_app_config();
    }

    pub(super) fn shortcuts_tab(&mut self, ui: &mut egui::Ui, scroll_height: f32) {
        let defaults = HotkeyBindings::default();
        let changed = self.app_config.keyboard_shortcuts != defaults;
        ui.horizontal(|ui| {
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.shortcut_search)
                    .hint_text(format!("{}  Search shortcuts", icons::MAGNIFYING_GLASS))
                    .desired_width(260.0),
            );
            // Typing a search must not be recorded as a new key.
            if search.gained_focus() || search.changed() {
                self.stop_hotkey_recording();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(changed, egui::Button::new("Restore All Defaults"))
                    .on_hover_text("Put every shortcut back to its original key")
                    .clicked()
                {
                    self.app_config.keyboard_shortcuts = HotkeyBindings::default();
                    self.stop_hotkey_recording();
                    self.save_app_config();
                }
            });
        });
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "Click a key to change it, then tap one key, or hold one and press a second. \
                 Mouse controls are not changed here.",
            )
            .size(12.5)
            .color(crate::ui_theme::SECONDARY),
        );
        ui.add_space(8.0);

        let query = self.shortcut_search.trim().to_owned();
        let mut any_shown = false;
        let mut viewpoints_open = true;
        egui::ScrollArea::vertical()
            .id_salt("settings_shortcuts_scroll")
            .max_height(scroll_height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (title, slots) in SHORTCUT_GROUPS {
                    let shown: Vec<HotkeySlot> = slots
                        .iter()
                        .copied()
                        .filter(|slot| {
                            matches_search(
                                *slot,
                                self.app_config.keyboard_shortcuts.get(*slot),
                                &query,
                            )
                        })
                        .collect();
                    if shown.is_empty() {
                        continue;
                    }
                    any_shown = true;
                    if title == VIEWPOINTS {
                        // Eleven rows most people never change: folded away
                        // unless a search finds one of them.
                        let header =
                            egui::CollapsingHeader::new(egui::RichText::new(title).strong())
                                .id_salt("settings_saved_viewpoint_shortcuts")
                                .default_open(false)
                                .open((!query.is_empty()).then_some(true))
                                .show(ui, |ui| self.shortcut_group(ui, &shown, &defaults));
                        viewpoints_open = header.body_response.is_some();
                    } else {
                        crate::ui_theme::section(ui, &title.to_uppercase());
                        self.shortcut_group(ui, &shown, &defaults);
                    }
                    ui.add_space(10.0);
                }
                if !any_shown {
                    ui.add_space(12.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(format!("No shortcuts match “{query}”."))
                                .color(crate::ui_theme::SECONDARY),
                        );
                    });
                }
            });
        // A row folded out of sight cannot keep listening.
        if !viewpoints_open
            && self.hotkey_recording.is_some_and(|slot| {
                SHORTCUT_GROUPS
                    .iter()
                    .any(|(title, slots)| *title == VIEWPOINTS && slots.contains(&slot))
            })
        {
            self.stop_hotkey_recording();
        }
    }

    /// One card of shortcut rows.
    fn shortcut_group(
        &mut self,
        ui: &mut egui::Ui,
        slots: &[HotkeySlot],
        defaults: &HotkeyBindings,
    ) {
        egui::Frame::NONE
            .fill(crate::ui_theme::PANEL)
            .stroke(egui::Stroke::new(1.0, crate::ui_theme::BORDER))
            .corner_radius(8.0)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (index, slot) in slots.iter().enumerate() {
                    if index > 0 {
                        ui.add(egui::Separator::default().spacing(2.0));
                    }
                    let action = self.shortcut_row(ui, *slot, defaults.get(*slot));
                    match action {
                        Some(RowAction::Record) => {
                            self.hotkey_recording = Some(*slot);
                            self.hotkey_recording_first = None;
                            self.hotkey_conflict = None;
                        }
                        Some(RowAction::Cancel) => self.stop_hotkey_recording(),
                        Some(RowAction::Reset) => {
                            self.stop_hotkey_recording();
                            let default = defaults.get(*slot);
                            match self.app_config.keyboard_shortcuts.conflict(*slot, default) {
                                Some(other) => {
                                    self.hotkey_conflict = Some(HotkeyConflict {
                                        slot: *slot,
                                        hotkey: default,
                                        other,
                                    });
                                }
                                None => {
                                    self.app_config.keyboard_shortcuts.set(*slot, default);
                                    self.save_app_config();
                                }
                            }
                        }
                        Some(RowAction::Swap(conflict)) => self.swap_hotkeys(conflict),
                        None => {}
                    }
                }
            });
    }

    fn shortcut_row(
        &self,
        ui: &mut egui::Ui,
        slot: HotkeySlot,
        default: Hotkey,
    ) -> Option<RowAction> {
        let current = self.app_config.keyboard_shortcuts.get(slot);
        let recording = self.hotkey_recording == Some(slot);
        let mut action = None;
        ui.horizontal(|ui| {
            ui.set_min_height(30.0);
            ui.label(slot.label());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // The last column is always the same width, so key buttons
                // line up whether or not a row shows Cancel or Reset.
                let icon_size = egui::vec2(ROW_ICON_WIDTH, 24.0);
                if recording {
                    if ui
                        .add_sized(icon_size, egui::Button::new(icons::X))
                        .on_hover_text("Cancel and keep the current key")
                        .clicked()
                    {
                        action = Some(RowAction::Cancel);
                    }
                } else if current != default {
                    if ui
                        .add_sized(icon_size, egui::Button::new(icons::ARROW_COUNTER_CLOCKWISE))
                        .on_hover_text(format!("Reset to {}", default.display()))
                        .clicked()
                    {
                        action = Some(RowAction::Reset);
                    }
                } else {
                    ui.allocate_space(icon_size);
                }

                let (text, fill, stroke) = if recording {
                    let text = self.hotkey_recording_first.map_or_else(
                        || "Press a key…".to_owned(),
                        |first| format!("{} + …", Hotkey::key(first).display()),
                    );
                    (
                        egui::RichText::new(text).color(crate::ui_theme::GOLD),
                        crate::ui_theme::GOLD_DARK,
                        egui::Stroke::new(1.0, crate::ui_theme::GOLD),
                    )
                } else {
                    (
                        crate::ui_theme::shortcut_text(current).size(13.5),
                        crate::ui_theme::SURFACE,
                        egui::Stroke::new(1.0, crate::ui_theme::BORDER),
                    )
                };
                let key = ui
                    .add(
                        egui::Button::new(text)
                            .fill(fill)
                            .stroke(stroke)
                            .min_size(egui::vec2(KEY_BUTTON_WIDTH, 26.0)),
                    )
                    .on_hover_text("Click, then press the new key");
                if key.clicked() && !recording {
                    action = Some(RowAction::Record);
                }
            });
        });
        if let Some(conflict) = self
            .hotkey_conflict
            .filter(|conflict| conflict.slot == slot)
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{} is already used by “{}”.",
                        conflict.hotkey.display(),
                        conflict.other.label()
                    ))
                    .color(crate::ui_theme::DANGER),
                );
                if ui
                    .button("Swap")
                    .on_hover_text(format!(
                        "Use {} here, and give “{}” {}",
                        conflict.hotkey.display(),
                        conflict.other.label(),
                        current.display()
                    ))
                    .clicked()
                {
                    action = Some(RowAction::Swap(conflict));
                }
            });
        } else if recording {
            ui.label(
                egui::RichText::new(
                    "Listening… release for one key, or hold it and press another.",
                )
                .size(12.0)
                .color(crate::ui_theme::SECONDARY),
            );
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shortcut_is_listed_exactly_once() {
        let listed: Vec<HotkeySlot> = SHORTCUT_GROUPS
            .iter()
            .flat_map(|(_, slots)| slots.iter().copied())
            .collect();
        assert_eq!(listed.len(), HotkeySlot::ALL.len());
        for slot in HotkeySlot::ALL {
            assert_eq!(
                listed.iter().filter(|listed| **listed == slot).count(),
                1,
                "{}",
                slot.label()
            );
        }
    }

    #[test]
    fn swapping_trades_keys_between_the_two_shortcuts() {
        let mut bindings = HotkeyBindings::default();
        let grid = bindings.get(HotkeySlot::ToggleGrid);
        let picker = bindings.get(HotkeySlot::ToggleWindowPicker);
        let conflict = HotkeyConflict {
            slot: HotkeySlot::ToggleGrid,
            hotkey: picker,
            other: bindings
                .conflict(HotkeySlot::ToggleGrid, picker)
                .expect("the picker has that key"),
        };
        swap(&mut bindings, conflict);
        assert_eq!(bindings.get(HotkeySlot::ToggleGrid), picker);
        assert_eq!(bindings.get(HotkeySlot::ToggleWindowPicker), grid);
        for slot in HotkeySlot::ALL {
            assert_eq!(
                bindings.conflict(slot, bindings.get(slot)),
                None,
                "{}",
                slot.label()
            );
        }
    }

    #[test]
    fn search_matches_names_and_keys_word_by_word() {
        let defaults = HotkeyBindings::default();
        let group = HotkeySlot::GroupTiles;
        let key = defaults.get(group);
        assert!(matches_search(group, key, ""));
        assert!(matches_search(group, key, "group"));
        assert!(matches_search(group, key, "SELECTED group"));
        assert!(matches_search(group, key, &key.display()));
        assert!(!matches_search(group, key, "record"));
        let grid = HotkeySlot::ToggleGrid;
        assert!(matches_search(grid, defaults.get(grid), "grid g"));
    }
}
