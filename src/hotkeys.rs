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
    UndoRemovedTiles,
    FocusCurrentTile,
    ExitTileOrBrowser,
    InteractBrowser,
    InteractBrowserAlternate,
    ShowShortcutHelp,
    GroupTiles,
    UngroupTiles,
    LayoutGrid,
    LayoutRow,
    LayoutColumn,
    LayoutSideStack,
    ToggleSpotlight,
    PreviousSpotlight,
    NextSpotlight,
    SizeMedium,
    SizeLarge,
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

    pub const ALL: [Self; 33] = [
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
        Self::UndoRemovedTiles,
        Self::FocusCurrentTile,
        Self::ExitTileOrBrowser,
        Self::InteractBrowser,
        Self::InteractBrowserAlternate,
        Self::ShowShortcutHelp,
        Self::GroupTiles,
        Self::UngroupTiles,
        Self::LayoutGrid,
        Self::LayoutRow,
        Self::LayoutColumn,
        Self::LayoutSideStack,
        Self::ToggleSpotlight,
        Self::PreviousSpotlight,
        Self::NextSpotlight,
        Self::SizeMedium,
        Self::SizeLarge,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::ToggleWindowPicker => "Toggle Window Picker",
            Self::ToggleGrid => "Toggle grid",
            Self::ToggleCanvasOnly => "Canvas-only mode",
            Self::PreviousCanvasView => "Previous saved viewpoint",
            Self::NextCanvasView => "Next saved viewpoint",
            Self::CanvasView1 => "Saved viewpoint 1",
            Self::CanvasView2 => "Saved viewpoint 2",
            Self::CanvasView3 => "Saved viewpoint 3",
            Self::CanvasView4 => "Saved viewpoint 4",
            Self::CanvasView5 => "Saved viewpoint 5",
            Self::CanvasView6 => "Saved viewpoint 6",
            Self::CanvasView7 => "Saved viewpoint 7",
            Self::CanvasView8 => "Saved viewpoint 8",
            Self::CanvasView9 => "Saved viewpoint 9",
            Self::SelectAll => "Select all tiles",
            Self::DeleteSelected => "Delete selected tiles",
            Self::UndoRemovedTiles => "Undo removed tiles",
            Self::FocusCurrentTile => "Focus current tile",
            Self::ExitTileOrBrowser => "Exit tile/browser mode",
            Self::InteractBrowser => "Interact with browser",
            Self::InteractBrowserAlternate => "Interact with browser (alternate)",
            Self::ShowShortcutHelp => "Show shortcut help",
            Self::GroupTiles => "Group selected tiles",
            Self::UngroupTiles => "Ungroup selected tiles",
            Self::LayoutGrid => "Layout: one large tile",
            Self::LayoutRow => "Layout: two large tiles",
            Self::LayoutColumn => "Layout: three large tiles",
            Self::LayoutSideStack => "Layout: equal tiles",
            Self::ToggleSpotlight => "Toggle group Spotlight",
            Self::PreviousSpotlight => "Previous Spotlight tile",
            Self::NextSpotlight => "Next Spotlight tile",
            Self::SizeMedium => "Medium group size",
            Self::SizeLarge => "Large group size",
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
    pub undo_removed_tiles: Hotkey,
    pub focus_current_tile: Hotkey,
    pub exit_tile_or_browser: Hotkey,
    pub interact_browser: Hotkey,
    pub interact_browser_alternate: Hotkey,
    pub show_shortcut_help: Hotkey,
    pub group_tiles: Hotkey,
    pub ungroup_tiles: Hotkey,
    pub layout_grid: Hotkey,
    pub layout_row: Hotkey,
    pub layout_column: Hotkey,
    pub layout_side_stack: Hotkey,
    pub toggle_spotlight: Hotkey,
    pub previous_spotlight: Hotkey,
    pub next_spotlight: Hotkey,
    pub size_medium: Hotkey,
    pub size_large: Hotkey,
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
            undo_removed_tiles: Hotkey::pair(0x11, 0x5A),   // Ctrl+Z
            focus_current_tile: Hotkey::key(0x46),          // F
            exit_tile_or_browser: Hotkey::key(0x1B),        // Escape
            interact_browser: Hotkey::pair(0x11, 0x42),     // Ctrl+B
            interact_browser_alternate: Hotkey::key(0x61),  // Numpad 1
            show_shortcut_help: Hotkey::key(0x70),          // F1
            group_tiles: Hotkey::pair(0x11, 0x47),
            ungroup_tiles: Hotkey::pair(0x11, 0x55),
            layout_grid: Hotkey::pair(0x12, 0x31),
            layout_row: Hotkey::pair(0x12, 0x32),
            layout_column: Hotkey::pair(0x12, 0x33),
            layout_side_stack: Hotkey::pair(0x12, 0x34),
            toggle_spotlight: Hotkey::pair(0x10, 0x46),
            previous_spotlight: Hotkey::pair(0x10, 0x25),
            next_spotlight: Hotkey::pair(0x10, 0x27),
            size_medium: Hotkey::pair(0x12, 0x36),
            size_large: Hotkey::pair(0x12, 0x37),
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
            HotkeySlot::UndoRemovedTiles => self.undo_removed_tiles,
            HotkeySlot::FocusCurrentTile => self.focus_current_tile,
            HotkeySlot::ExitTileOrBrowser => self.exit_tile_or_browser,
            HotkeySlot::InteractBrowser => self.interact_browser,
            HotkeySlot::InteractBrowserAlternate => self.interact_browser_alternate,
            HotkeySlot::ShowShortcutHelp => self.show_shortcut_help,
            HotkeySlot::GroupTiles => self.group_tiles,
            HotkeySlot::UngroupTiles => self.ungroup_tiles,
            HotkeySlot::LayoutGrid => self.layout_grid,
            HotkeySlot::LayoutRow => self.layout_row,
            HotkeySlot::LayoutColumn => self.layout_column,
            HotkeySlot::LayoutSideStack => self.layout_side_stack,
            HotkeySlot::ToggleSpotlight => self.toggle_spotlight,
            HotkeySlot::PreviousSpotlight => self.previous_spotlight,
            HotkeySlot::NextSpotlight => self.next_spotlight,
            HotkeySlot::SizeMedium => self.size_medium,
            HotkeySlot::SizeLarge => self.size_large,
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
            HotkeySlot::UndoRemovedTiles => self.undo_removed_tiles = hotkey,
            HotkeySlot::FocusCurrentTile => self.focus_current_tile = hotkey,
            HotkeySlot::ExitTileOrBrowser => self.exit_tile_or_browser = hotkey,
            HotkeySlot::InteractBrowser => self.interact_browser = hotkey,
            HotkeySlot::InteractBrowserAlternate => self.interact_browser_alternate = hotkey,
            HotkeySlot::ShowShortcutHelp => self.show_shortcut_help = hotkey,
            HotkeySlot::GroupTiles => self.group_tiles = hotkey,
            HotkeySlot::UngroupTiles => self.ungroup_tiles = hotkey,
            HotkeySlot::LayoutGrid => self.layout_grid = hotkey,
            HotkeySlot::LayoutRow => self.layout_row = hotkey,
            HotkeySlot::LayoutColumn => self.layout_column = hotkey,
            HotkeySlot::LayoutSideStack => self.layout_side_stack = hotkey,
            HotkeySlot::ToggleSpotlight => self.toggle_spotlight = hotkey,
            HotkeySlot::PreviousSpotlight => self.previous_spotlight = hotkey,
            HotkeySlot::NextSpotlight => self.next_spotlight = hotkey,
            HotkeySlot::SizeMedium => self.size_medium = hotkey,
            HotkeySlot::SizeLarge => self.size_large = hotkey,
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

#[derive(Clone, Copy, Debug)]
pub struct HotkeyPresses([bool; HotkeySlot::ALL.len()]);

impl Default for HotkeyPresses {
    fn default() -> Self {
        Self([false; HotkeySlot::ALL.len()])
    }
}

impl HotkeyPresses {
    pub fn pressed(self, slot: HotkeySlot) -> bool {
        self.0[slot as usize]
    }

    pub fn for_browser_interaction(mut self, active: bool) -> Self {
        if active {
            // Native WebView text fields do not report keyboard ownership to
            // egui. Leave page keys alone except for explicitly exiting or
            // toggling browser interaction.
            for slot in HotkeySlot::ALL {
                if !matches!(
                    slot,
                    HotkeySlot::ExitTileOrBrowser
                        | HotkeySlot::InteractBrowser
                        | HotkeySlot::InteractBrowserAlternate
                ) {
                    self.0[slot as usize] = false;
                }
            }
        }
        self
    }
}

pub struct HotkeyTracker {
    held: [bool; 256],
    newly_pressed: Vec<u16>,
    /// When Pluriview last sampled while owning the foreground.
    last_foreground_sample: Option<std::time::Instant>,
    /// Edges are only trusted while Pluriview has owned the foreground
    /// since the previous sample; see `key_press_edge`.
    accept_edges: bool,
}

impl Default for HotkeyTracker {
    fn default() -> Self {
        Self {
            held: [false; 256],
            newly_pressed: Vec::new(),
            last_foreground_sample: None,
            accept_edges: false,
        }
    }
}

/// Longest gap between foreground samples that still counts as continuous
/// ownership. Idle frames run at least every 250 ms.
const FOREGROUND_CONTINUITY: std::time::Duration = std::time::Duration::from_secs(1);

/// Decide whether a sampled `GetAsyncKeyState` value is a new press.
///
/// The low "pressed since the last call" bit is system-wide, so it is also
/// set by keys typed in other applications. Only trust edges while Pluriview
/// owned the foreground at both this and the previous sample; on the frame
/// focus returns, just record which keys are already down.
fn key_press_edge(state: u16, held: bool, accept_edges: bool) -> bool {
    let down = state & 0x8000 != 0;
    let pressed_since_sample = state & 1 != 0;
    accept_edges && ((down && !held) || (!down && pressed_since_sample))
}

impl HotkeyTracker {
    pub fn sample(&mut self, bindings: &HotkeyBindings, recording: bool, foreground: bool) {
        self.newly_pressed.clear();
        // A long gap (for example while minimized, when no frames run) also
        // breaks continuity even if the previous sample owned the foreground.
        let now = std::time::Instant::now();
        self.accept_edges = foreground
            && self
                .last_foreground_sample
                .is_some_and(|last| now.duration_since(last) < FOREGROUND_CONTINUITY);
        self.last_foreground_sample = foreground.then_some(now);
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
            let _ = (bindings, recording, foreground);
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
        if key_press_edge(state, self.held[index], self.accept_edges) {
            self.newly_pressed.push(virtual_key);
        }
        self.held[index] = state & 0x8000 != 0;
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
    #[test]
    fn key_edges_from_other_apps_never_become_shortcuts() {
        use super::key_press_edge;
        const DOWN: u16 = 0x8000;
        const PRESSED_SINCE_LAST_CALL: u16 = 1;

        // Continuous foreground ownership: real presses, including a quick
        // tap between samples, are accepted.
        assert!(key_press_edge(DOWN, false, true));
        assert!(key_press_edge(PRESSED_SINCE_LAST_CALL, false, true));
        assert!(!key_press_edge(DOWN, true, true));

        // Delete typed in another app before focus returned: neither the
        // low bit nor a still-held key fires on the first focused sample.
        assert!(!key_press_edge(PRESSED_SINCE_LAST_CALL, false, false));
        assert!(!key_press_edge(
            DOWN | PRESSED_SINCE_LAST_CALL,
            false,
            false
        ));
    }

    #[test]
    fn undo_defaults_for_legacy_settings_and_respects_keyboard_focus() {
        let mut value = serde_json::to_value(super::HotkeyBindings::default()).unwrap();
        value.as_object_mut().unwrap().remove("undo_removed_tiles");
        let bindings: super::HotkeyBindings = serde_json::from_value(value).unwrap();
        assert_eq!(bindings.undo_removed_tiles, super::Hotkey::pair(0x11, 0x5A));
        let mut tracker = super::HotkeyTracker::default();
        tracker.held[0x11] = true;
        tracker.held[0x5A] = true;
        tracker.newly_pressed.push(0x5A);
        assert!(tracker
            .presses(&bindings, true)
            .pressed(super::HotkeySlot::UndoRemovedTiles));
        assert!(!tracker
            .presses(&bindings, false)
            .pressed(super::HotkeySlot::UndoRemovedTiles));
        tracker.newly_pressed.clear();
        assert!(!tracker
            .presses(&bindings, true)
            .pressed(super::HotkeySlot::UndoRemovedTiles));
    }

    use super::{Hotkey, HotkeyBindings, HotkeySlot, HotkeyTracker};

    #[test]
    fn default_keyboard_shortcuts() {
        let shortcuts = HotkeyBindings::default();
        assert_eq!(shortcuts.toggle_window_picker.display(), "W");
        assert_eq!(shortcuts.toggle_grid.display(), "G");
        assert_eq!(shortcuts.select_all.display(), "Ctrl+A");
        assert_eq!(shortcuts.focus_current_tile.display(), "F");
        assert_eq!(shortcuts.interact_browser_alternate.display(), "Numpad 1");
        assert_eq!(shortcuts.previous_canvas_view.display(), "Ctrl+Page Up");
        assert_eq!(shortcuts.next_canvas_view.display(), "Ctrl+Page Down");
        assert_eq!(shortcuts.canvas_view_1.display(), "Ctrl+1");
        assert_eq!(shortcuts.canvas_view_9.display(), "Ctrl+9");
    }

    #[test]
    fn spotlight_presets_keep_saved_bindings_and_remove_alt_five() {
        let old = serde_json::json!({
            "layout_row": {"first_key": 17, "second_key": 81},
            "size_small": {"first_key": 18, "second_key": 53}
        });
        let restored: HotkeyBindings = serde_json::from_value(old).unwrap();
        assert_eq!(restored.layout_row, Hotkey::pair(0x11, 0x51));
        let defaults = HotkeyBindings::default();
        for (slot, number) in [
            (HotkeySlot::LayoutGrid, 0x31),
            (HotkeySlot::LayoutRow, 0x32),
            (HotkeySlot::LayoutColumn, 0x33),
            (HotkeySlot::LayoutSideStack, 0x34),
        ] {
            assert_eq!(defaults.get(slot), Hotkey::pair(0x12, number));
        }
        assert!(!HotkeySlot::ALL
            .iter()
            .any(|slot| defaults.get(*slot) == Hotkey::pair(0x12, 0x35)));
        assert!(serde_json::to_value(restored)
            .unwrap()
            .get("size_small")
            .is_none());
    }

    #[test]
    fn group_shortcuts_migrate_without_changing_existing_bindings() {
        let old = serde_json::json!({"focus_current_tile": {"first_key": null, "second_key": 98}});
        let restored: HotkeyBindings = serde_json::from_value(old).unwrap();
        assert_eq!(restored.focus_current_tile, Hotkey::key(0x62));
        assert_eq!(restored.group_tiles.display(), "Ctrl+G");
        assert_eq!(restored.toggle_spotlight.display(), "Shift+F");
        let defaults = HotkeyBindings::default();
        for slot in HotkeySlot::ALL {
            assert_eq!(defaults.conflict(slot, defaults.get(slot)), None);
        }
        let mut tracker = HotkeyTracker::default();
        tracker.held[0x10] = true;
        tracker.held[0x46] = true;
        tracker.newly_pressed.push(0x46);
        let presses = tracker.presses(&defaults, true);
        assert!(presses.pressed(HotkeySlot::ToggleSpotlight));
        assert!(!presses.pressed(HotkeySlot::FocusCurrentTile));
        assert!(!tracker
            .presses(&defaults, false)
            .pressed(HotkeySlot::ToggleSpotlight));
    }

    #[test]
    fn focus_defaults_to_f_when_missing_from_saved_settings() {
        let mut value = serde_json::to_value(HotkeyBindings::default()).unwrap();
        value.as_object_mut().unwrap().remove("focus_current_tile");
        let bindings: HotkeyBindings = serde_json::from_value(value).unwrap();
        assert_eq!(bindings.focus_current_tile, Hotkey::key(0x46));
    }

    #[test]
    fn saved_focus_bindings_are_preserved() {
        for hotkey in [Hotkey::key(0x62), Hotkey::pair(0x11, 0x46)] {
            let bindings = HotkeyBindings {
                focus_current_tile: hotkey,
                ..Default::default()
            };
            let value = serde_json::to_value(bindings).unwrap();
            let restored: HotkeyBindings = serde_json::from_value(value).unwrap();
            assert_eq!(restored.focus_current_tile, hotkey);
        }
    }

    #[test]
    fn focus_shortcut_respects_keyboard_ownership() {
        let bindings = HotkeyBindings::default();
        assert_eq!(
            bindings.conflict(HotkeySlot::FocusCurrentTile, bindings.focus_current_tile),
            None
        );
        let mut tracker = HotkeyTracker::default();
        tracker.held[0x46] = true;
        tracker.newly_pressed.push(0x46);
        assert!(tracker
            .presses(&bindings, true)
            .pressed(HotkeySlot::FocusCurrentTile));
        assert!(!tracker
            .presses(&bindings, false)
            .pressed(HotkeySlot::FocusCurrentTile));
    }

    #[test]
    fn interactive_browser_keeps_page_keys_and_preserves_exit_controls() {
        let mut bindings = HotkeyBindings::default();
        for focus in [
            Hotkey::key(0x46),
            Hotkey::key(0x62),
            Hotkey::pair(0x11, 0x46),
        ] {
            bindings.focus_current_tile = focus;
            for slot in HotkeySlot::ALL {
                let binding = bindings.get(slot);
                let mut tracker = HotkeyTracker::default();
                tracker.held[usize::from(binding.second_key)] = true;
                tracker.newly_pressed.push(binding.second_key);
                if let Some(first_key) = binding.first_key {
                    tracker.held[usize::from(first_key)] = true;
                }
                let presses = tracker.presses(&bindings, true);
                assert!(presses.for_browser_interaction(false).pressed(slot));
                let browser_control = matches!(
                    slot,
                    HotkeySlot::ExitTileOrBrowser
                        | HotkeySlot::InteractBrowser
                        | HotkeySlot::InteractBrowserAlternate
                );
                assert_eq!(
                    presses.for_browser_interaction(true).pressed(slot),
                    browser_control,
                    "{slot:?}"
                );
            }
        }
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
            28
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
