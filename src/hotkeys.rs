use serde::{Deserialize, Serialize};

/// One keyboard key, or an ordered pair pressed together. Mouse buttons are
/// deliberately absent: pointer gestures remain fixed canvas controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Hotkey {
    pub first_key: Option<u16>,
    pub second_key: u16,
}

impl Hotkey {
    pub const fn key(virtual_key: u16) -> Self {
        Self {
            first_key: None,
            second_key: virtual_key,
        }
    }

    pub const fn pair(first_key: u16, second_key: u16) -> Self {
        if first_key <= second_key {
            Self {
                first_key: Some(first_key),
                second_key,
            }
        } else {
            Self {
                first_key: Some(second_key),
                second_key: first_key,
            }
        }
    }

    pub fn display(self) -> String {
        match self.first_key {
            Some(first_key) => format!("{}+{}", key_label(first_key), key_label(self.second_key)),
            None => key_label(self.second_key).to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum HotkeySlot {
    ToggleWindowPicker,
    ToggleGrid,
    ToggleCanvasOnly,
    PreviousCanvasView,
    NextCanvasView,
    CanvasView1,
    CanvasView2,
    CanvasView3,
    CanvasView4,
    CanvasView5,
    CanvasView6,
    CanvasView7,
    CanvasView8,
    CanvasView9,
    SelectAll,
    DeleteSelected,
    FocusCurrentTile,
    ExitTileOrBrowser,
    InteractBrowser,
    InteractBrowserAlternate,
    ShowShortcutHelp,
}

impl HotkeySlot {
    pub const CANVAS_VIEWS: [Self; 9] = [
        Self::CanvasView1,
        Self::CanvasView2,
        Self::CanvasView3,
        Self::CanvasView4,
        Self::CanvasView5,
        Self::CanvasView6,
        Self::CanvasView7,
        Self::CanvasView8,
        Self::CanvasView9,
    ];

    pub const ALL: [Self; 21] = [
        Self::ToggleWindowPicker,
        Self::ToggleGrid,
        Self::ToggleCanvasOnly,
        Self::PreviousCanvasView,
        Self::NextCanvasView,
        Self::CanvasView1,
        Self::CanvasView2,
        Self::CanvasView3,
        Self::CanvasView4,
        Self::CanvasView5,
        Self::CanvasView6,
        Self::CanvasView7,
        Self::CanvasView8,
        Self::CanvasView9,
        Self::SelectAll,
        Self::DeleteSelected,
        Self::FocusCurrentTile,
        Self::ExitTileOrBrowser,
        Self::InteractBrowser,
        Self::InteractBrowserAlternate,
        Self::ShowShortcutHelp,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::ToggleWindowPicker => "Toggle Window Picker",
            Self::ToggleGrid => "Toggle grid",
            Self::ToggleCanvasOnly => "Canvas-only mode",
            Self::PreviousCanvasView => "Previous canvas view",
            Self::NextCanvasView => "Next canvas view",
            Self::CanvasView1 => "Canvas view 1",
            Self::CanvasView2 => "Canvas view 2",
            Self::CanvasView3 => "Canvas view 3",
            Self::CanvasView4 => "Canvas view 4",
            Self::CanvasView5 => "Canvas view 5",
            Self::CanvasView6 => "Canvas view 6",
            Self::CanvasView7 => "Canvas view 7",
            Self::CanvasView8 => "Canvas view 8",
            Self::CanvasView9 => "Canvas view 9",
            Self::SelectAll => "Select all tiles",
            Self::DeleteSelected => "Delete selected tiles",
            Self::FocusCurrentTile => "Focus current tile",
            Self::ExitTileOrBrowser => "Exit tile/browser mode",
            Self::InteractBrowser => "Interact with browser",
            Self::InteractBrowserAlternate => "Interact with browser (alternate)",
            Self::ShowShortcutHelp => "Show shortcut help",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct HotkeyBindings {
    pub toggle_window_picker: Hotkey,
    pub toggle_grid: Hotkey,
    pub toggle_canvas_only: Hotkey,
    pub previous_canvas_view: Hotkey,
    pub next_canvas_view: Hotkey,
    pub canvas_view_1: Hotkey,
    pub canvas_view_2: Hotkey,
    pub canvas_view_3: Hotkey,
    pub canvas_view_4: Hotkey,
    pub canvas_view_5: Hotkey,
    pub canvas_view_6: Hotkey,
    pub canvas_view_7: Hotkey,
    pub canvas_view_8: Hotkey,
    pub canvas_view_9: Hotkey,
    pub select_all: Hotkey,
    pub delete_selected: Hotkey,
    pub focus_current_tile: Hotkey,
    pub exit_tile_or_browser: Hotkey,
    pub interact_browser: Hotkey,
    pub interact_browser_alternate: Hotkey,
    pub show_shortcut_help: Hotkey,
}

impl Default for HotkeyBindings {
    fn default() -> Self {
        Self {
            toggle_window_picker: Hotkey::key(0x57),        // W
            toggle_grid: Hotkey::key(0x47),                 // G
            toggle_canvas_only: Hotkey::key(0x48),          // H
            previous_canvas_view: Hotkey::pair(0x11, 0x21), // Ctrl+Page Up
            next_canvas_view: Hotkey::pair(0x11, 0x22),     // Ctrl+Page Down
            canvas_view_1: Hotkey::pair(0x11, 0x31),        // Ctrl+1
            canvas_view_2: Hotkey::pair(0x11, 0x32),        // Ctrl+2
            canvas_view_3: Hotkey::pair(0x11, 0x33),        // Ctrl+3
            canvas_view_4: Hotkey::pair(0x11, 0x34),        // Ctrl+4
            canvas_view_5: Hotkey::pair(0x11, 0x35),        // Ctrl+5
            canvas_view_6: Hotkey::pair(0x11, 0x36),        // Ctrl+6
            canvas_view_7: Hotkey::pair(0x11, 0x37),        // Ctrl+7
            canvas_view_8: Hotkey::pair(0x11, 0x38),        // Ctrl+8
            canvas_view_9: Hotkey::pair(0x11, 0x39),        // Ctrl+9
            select_all: Hotkey::pair(0x11, 0x41),           // Ctrl+A
            delete_selected: Hotkey::key(0x2E),             // Delete
            focus_current_tile: Hotkey::key(0x62),          // Numpad 2
            exit_tile_or_browser: Hotkey::key(0x1B),        // Escape
            interact_browser: Hotkey::pair(0x11, 0x42),     // Ctrl+B
            interact_browser_alternate: Hotkey::key(0x61),  // Numpad 1
            show_shortcut_help: Hotkey::key(0x70),          // F1
        }
    }
}

impl HotkeyBindings {
    pub fn get(&self, slot: HotkeySlot) -> Hotkey {
        match slot {
            HotkeySlot::ToggleWindowPicker => self.toggle_window_picker,
            HotkeySlot::ToggleGrid => self.toggle_grid,
            HotkeySlot::ToggleCanvasOnly => self.toggle_canvas_only,
            HotkeySlot::PreviousCanvasView => self.previous_canvas_view,
            HotkeySlot::NextCanvasView => self.next_canvas_view,
            HotkeySlot::CanvasView1 => self.canvas_view_1,
            HotkeySlot::CanvasView2 => self.canvas_view_2,
            HotkeySlot::CanvasView3 => self.canvas_view_3,
            HotkeySlot::CanvasView4 => self.canvas_view_4,
            HotkeySlot::CanvasView5 => self.canvas_view_5,
            HotkeySlot::CanvasView6 => self.canvas_view_6,
            HotkeySlot::CanvasView7 => self.canvas_view_7,
            HotkeySlot::CanvasView8 => self.canvas_view_8,
            HotkeySlot::CanvasView9 => self.canvas_view_9,
            HotkeySlot::SelectAll => self.select_all,
            HotkeySlot::DeleteSelected => self.delete_selected,
            HotkeySlot::FocusCurrentTile => self.focus_current_tile,
            HotkeySlot::ExitTileOrBrowser => self.exit_tile_or_browser,
            HotkeySlot::InteractBrowser => self.interact_browser,
            HotkeySlot::InteractBrowserAlternate => self.interact_browser_alternate,
            HotkeySlot::ShowShortcutHelp => self.show_shortcut_help,
        }
    }

    pub fn set(&mut self, slot: HotkeySlot, hotkey: Hotkey) {
        match slot {
            HotkeySlot::ToggleWindowPicker => self.toggle_window_picker = hotkey,
            HotkeySlot::ToggleGrid => self.toggle_grid = hotkey,
            HotkeySlot::ToggleCanvasOnly => self.toggle_canvas_only = hotkey,
            HotkeySlot::PreviousCanvasView => self.previous_canvas_view = hotkey,
            HotkeySlot::NextCanvasView => self.next_canvas_view = hotkey,
            HotkeySlot::CanvasView1 => self.canvas_view_1 = hotkey,
            HotkeySlot::CanvasView2 => self.canvas_view_2 = hotkey,
            HotkeySlot::CanvasView3 => self.canvas_view_3 = hotkey,
            HotkeySlot::CanvasView4 => self.canvas_view_4 = hotkey,
            HotkeySlot::CanvasView5 => self.canvas_view_5 = hotkey,
            HotkeySlot::CanvasView6 => self.canvas_view_6 = hotkey,
            HotkeySlot::CanvasView7 => self.canvas_view_7 = hotkey,
            HotkeySlot::CanvasView8 => self.canvas_view_8 = hotkey,
            HotkeySlot::CanvasView9 => self.canvas_view_9 = hotkey,
            HotkeySlot::SelectAll => self.select_all = hotkey,
            HotkeySlot::DeleteSelected => self.delete_selected = hotkey,
            HotkeySlot::FocusCurrentTile => self.focus_current_tile = hotkey,
            HotkeySlot::ExitTileOrBrowser => self.exit_tile_or_browser = hotkey,
            HotkeySlot::InteractBrowser => self.interact_browser = hotkey,
            HotkeySlot::InteractBrowserAlternate => self.interact_browser_alternate = hotkey,
            HotkeySlot::ShowShortcutHelp => self.show_shortcut_help = hotkey,
        }
    }

    pub fn conflict(&self, changed: HotkeySlot, candidate: Hotkey) -> Option<HotkeySlot> {
        HotkeySlot::ALL
            .into_iter()
            .find(|slot| *slot != changed && self.get(*slot) == candidate)
    }

    fn configured_keys(&self) -> [bool; 256] {
        let mut keys = [false; 256];
        for slot in HotkeySlot::ALL {
            let binding = self.get(slot);
            if let Some(key) = keys.get_mut(usize::from(binding.second_key)) {
                *key = true;
            }
            if let Some(first_key) = binding.first_key {
                if let Some(key) = keys.get_mut(usize::from(first_key)) {
                    *key = true;
                }
            }
        }
        keys
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HotkeyPresses([bool; HotkeySlot::ALL.len()]);

impl HotkeyPresses {
    pub fn pressed(self, slot: HotkeySlot) -> bool {
        self.0[slot as usize]
    }
}

pub struct HotkeyTracker {
    held: [bool; 256],
    newly_pressed: Vec<u16>,
}

impl Default for HotkeyTracker {
    fn default() -> Self {
        Self {
            held: [false; 256],
            newly_pressed: Vec::new(),
        }
    }
}

impl HotkeyTracker {
    pub fn sample(&mut self, bindings: &HotkeyBindings, recording: bool) {
        self.newly_pressed.clear();
        #[cfg(windows)]
        {
            let configured = bindings.configured_keys();
            let mut sampled = [false; 256];

            if recording {
                self.sample_supported_keys(&mut sampled, None);
            } else {
                for (virtual_key, enabled) in configured.iter().copied().enumerate() {
                    if enabled {
                        self.sample_key(virtual_key as u16, &mut sampled);
                    }
                }

                // A configured-key edge is the only time a shortcut can fire.
                // Validate every other supported key on that frame so an extra
                // held key still suppresses the shortcut, while idle frames poll
                // only the keys that can actually trigger an action.
                if !self.newly_pressed.is_empty() {
                    self.sample_supported_keys(&mut sampled, Some(&configured));
                }
            }

            for (virtual_key, was_sampled) in sampled.into_iter().enumerate() {
                if !was_sampled {
                    self.held[virtual_key] = false;
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (bindings, recording);
            self.held.fill(false);
        }
    }

    #[cfg(windows)]
    fn sample_supported_keys(&mut self, sampled: &mut [bool; 256], skip: Option<&[bool; 256]>) {
        for &(virtual_key, _) in SUPPORTED_KEYS {
            if skip.is_some_and(|keys| keys[usize::from(virtual_key)]) {
                continue;
            }
            self.sample_key(virtual_key, sampled);
        }
    }

    #[cfg(windows)]
    fn sample_key(&mut self, virtual_key: u16, sampled: &mut [bool; 256]) {
        let index = usize::from(virtual_key);
        if sampled[index] {
            return;
        }
        let state = key_state(virtual_key);
        let down = state & 0x8000 != 0;
        let pressed_since_sample = state & 1 != 0;
        if (down && !self.held[index]) || (!down && pressed_since_sample) {
            self.newly_pressed.push(virtual_key);
        }
        self.held[index] = down;
        sampled[index] = true;
    }

    pub fn presses(&self, bindings: &HotkeyBindings, listening: bool) -> HotkeyPresses {
        let mut presses = HotkeyPresses::default();
        if listening {
            for slot in HotkeySlot::ALL {
                presses.0[slot as usize] = self.binding_pressed(bindings.get(slot));
            }
        }
        presses
    }

    pub fn newly_pressed_key_except(&self, except: Option<u16>) -> Option<u16> {
        self.newly_pressed
            .iter()
            .copied()
            .find(|key| Some(*key) != except)
    }

    pub fn is_down(&self, virtual_key: u16) -> bool {
        self.held
            .get(usize::from(virtual_key))
            .copied()
            .unwrap_or(false)
    }

    fn binding_pressed(&self, binding: Hotkey) -> bool {
        let pressed = match binding.first_key {
            Some(first_key) => {
                (self.newly_pressed.contains(&binding.second_key) && self.is_down(first_key))
                    || (self.newly_pressed.contains(&first_key) && self.is_down(binding.second_key))
            }
            None => self.newly_pressed.contains(&binding.second_key),
        };
        pressed
            && self.held.iter().enumerate().all(|(virtual_key, down)| {
                !*down
                    || virtual_key == usize::from(binding.second_key)
                    || binding.first_key == u16::try_from(virtual_key).ok()
            })
    }
}

#[cfg(windows)]
fn key_state(virtual_key: u16) -> u16 {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    unsafe { GetAsyncKeyState(i32::from(virtual_key)) as u16 }
}

fn key_label(virtual_key: u16) -> &'static str {
    SUPPORTED_KEYS
        .iter()
        .find_map(|(key, label)| (*key == virtual_key).then_some(*label))
        .unwrap_or("Unknown key")
}

/// Keys offered by the recorder. Any two of these can form a pair.
const SUPPORTED_KEYS: &[(u16, &str)] = &[
    (0x08, "Backspace"),
    (0x09, "Tab"),
    (0x0D, "Enter"),
    (0x10, "Shift"),
    (0x11, "Ctrl"),
    (0x12, "Alt"),
    (0x1B, "Esc"),
    (0x20, "Space"),
    (0x21, "Page Up"),
    (0x22, "Page Down"),
    (0x23, "End"),
    (0x24, "Home"),
    (0x25, "Left"),
    (0x26, "Up"),
    (0x27, "Right"),
    (0x28, "Down"),
    (0x2D, "Insert"),
    (0x2E, "Delete"),
    (0x30, "0"),
    (0x31, "1"),
    (0x32, "2"),
    (0x33, "3"),
    (0x34, "4"),
    (0x35, "5"),
    (0x36, "6"),
    (0x37, "7"),
    (0x38, "8"),
    (0x39, "9"),
    (0x41, "A"),
    (0x42, "B"),
    (0x43, "C"),
    (0x44, "D"),
    (0x45, "E"),
    (0x46, "F"),
    (0x47, "G"),
    (0x48, "H"),
    (0x49, "I"),
    (0x4A, "J"),
    (0x4B, "K"),
    (0x4C, "L"),
    (0x4D, "M"),
    (0x4E, "N"),
    (0x4F, "O"),
    (0x50, "P"),
    (0x51, "Q"),
    (0x52, "R"),
    (0x53, "S"),
    (0x54, "T"),
    (0x55, "U"),
    (0x56, "V"),
    (0x57, "W"),
    (0x58, "X"),
    (0x59, "Y"),
    (0x5A, "Z"),
    (0x60, "Numpad 0"),
    (0x61, "Numpad 1"),
    (0x62, "Numpad 2"),
    (0x63, "Numpad 3"),
    (0x64, "Numpad 4"),
    (0x65, "Numpad 5"),
    (0x66, "Numpad 6"),
    (0x67, "Numpad 7"),
    (0x68, "Numpad 8"),
    (0x69, "Numpad 9"),
    (0x6A, "Numpad *"),
    (0x6B, "Numpad +"),
    (0x6D, "Numpad -"),
    (0x6E, "Numpad ."),
    (0x6F, "Numpad /"),
    (0x70, "F1"),
    (0x71, "F2"),
    (0x72, "F3"),
    (0x73, "F4"),
    (0x74, "F5"),
    (0x75, "F6"),
    (0x76, "F7"),
    (0x77, "F8"),
    (0x78, "F9"),
    (0x79, "F10"),
    (0x7A, "F11"),
    (0x7B, "F12"),
    (0xBA, ";"),
    (0xBB, "+"),
    (0xBC, ","),
    (0xBD, "-"),
    (0xBE, "."),
    (0xBF, "/"),
    (0xC0, "`"),
    (0xDB, "["),
    (0xDC, "\\"),
    (0xDD, "]"),
    (0xDE, "'"),
];

#[cfg(test)]
mod tests {
    use super::{Hotkey, HotkeyBindings, HotkeySlot, HotkeyTracker};

    #[test]
    fn defaults_preserve_existing_keyboard_shortcuts() {
        let shortcuts = HotkeyBindings::default();
        assert_eq!(shortcuts.toggle_window_picker.display(), "W");
        assert_eq!(shortcuts.toggle_grid.display(), "G");
        assert_eq!(shortcuts.select_all.display(), "Ctrl+A");
        assert_eq!(shortcuts.focus_current_tile.display(), "Numpad 2");
        assert_eq!(shortcuts.interact_browser_alternate.display(), "Numpad 1");
        assert_eq!(shortcuts.previous_canvas_view.display(), "Ctrl+Page Up");
        assert_eq!(shortcuts.next_canvas_view.display(), "Ctrl+Page Down");
        assert_eq!(shortcuts.canvas_view_1.display(), "Ctrl+1");
        assert_eq!(shortcuts.canvas_view_9.display(), "Ctrl+9");
    }

    #[test]
    fn duplicate_binding_reports_the_other_action() {
        let shortcuts = HotkeyBindings::default();
        assert_eq!(
            shortcuts.conflict(HotkeySlot::ToggleGrid, shortcuts.delete_selected),
            Some(HotkeySlot::DeleteSelected)
        );
    }

    #[test]
    fn two_key_order_is_normalized_for_conflict_detection() {
        assert_eq!(Hotkey::pair(0x51, 0x41), Hotkey::pair(0x41, 0x51));
    }

    #[test]
    fn arbitrary_two_key_bindings_are_supported() {
        let hotkey = Hotkey::pair(0x41, 0x51);
        assert_eq!(hotkey.display(), "A+Q");
        let json = serde_json::to_string(&hotkey).unwrap();
        assert_eq!(serde_json::from_str::<Hotkey>(&json).unwrap(), hotkey);
    }

    #[test]
    fn configured_key_scan_contains_only_bound_keys() {
        let shortcuts = HotkeyBindings::default();
        let configured = shortcuts.configured_keys();
        assert_eq!(
            configured.into_iter().filter(|enabled| *enabled).count(),
            22
        );
        assert!(configured[0x11]); // Ctrl
        assert!(configured[0x31]); // 1
        assert!(configured[0x39]); // 9
        assert!(configured[0x47]); // G
        assert!(configured[0x57]); // W
        assert!(!configured[0x51]); // Q
    }

    #[test]
    fn single_key_does_not_fire_with_an_extra_key_held() {
        let mut tracker = HotkeyTracker::default();
        tracker.newly_pressed.push(0x47);
        tracker.held[0x47] = true;
        assert!(tracker.binding_pressed(Hotkey::key(0x47)));
        tracker.held[0x11] = true;
        assert!(!tracker.binding_pressed(Hotkey::key(0x47)));
    }

    #[test]
    fn pair_fires_when_second_key_is_new() {
        let mut tracker = HotkeyTracker::default();
        tracker.held[0x41] = true;
        tracker.held[0x51] = true;
        tracker.newly_pressed.push(0x51);
        assert!(tracker.binding_pressed(Hotkey::pair(0x41, 0x51)));
    }
}
