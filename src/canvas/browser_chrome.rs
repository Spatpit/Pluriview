//! The toolbar and vertical tab strip drawn around a browser tile's page.

use super::*;
use crate::preview::browser_chrome::ChromeLayout;
use crate::preview::BrowserTab;
use crate::ui_theme;
use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat, TextWrapping};

/// An address typed into a browser tile's address bar, consumed by the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserNavigation {
    pub input: String,
    /// Open in a new tab rather than replacing the current page.
    pub new_tab: bool,
}

/// The address bar being typed in.
#[derive(Clone)]
pub(super) struct AddressEdit {
    id: PreviewId,
    text: String,
    new_tab: bool,
    focused: bool,
    /// Selection (character indices) to apply when the field takes focus;
    /// None selects everything.
    focus_selection: Option<(usize, usize)>,
    /// The selection the right-click menu acts on, kept from before the
    /// click because egui moves the text cursor on any press.
    selection: (usize, usize),
}

impl AddressEdit {
    fn new(id: PreviewId, text: String, new_tab: bool) -> Self {
        Self {
            id,
            text,
            new_tab,
            focused: false,
            focus_selection: None,
            selection: (0, 0),
        }
    }
}

/// A right-click menu choice in the address bar while typing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditCommand {
    Cut,
    Copy,
    Paste,
    PasteAndGo,
    SelectAll,
}

/// Requests from the frame that change the tile itself.
#[derive(Default)]
pub(super) struct ChromeOutput {
    pub close_tile: bool,
    pub toggle_tabs_collapsed: bool,
}

/// Sizes in tile units; the layout's scale converts them to screen points.
const BUTTON: f32 = 26.0;
const PAD: f32 = 5.0;
const TAB_HEIGHT: f32 = 30.0;
const TAB_GAP: f32 = 2.0;
const BADGE: f32 = 16.0;
/// Below this scale text is unreadable, so only the frame's shapes are drawn.
const MIN_DETAIL_SCALE: f32 = 0.4;

const FIELD_FILL: Color32 = Color32::from_rgb(39, 40, 46);
const DISABLED: Color32 = Color32::from_rgb(88, 89, 96);
const BADGE_COLORS: [Color32; 8] = [
    Color32::from_rgb(86, 110, 180),
    Color32::from_rgb(64, 138, 118),
    Color32::from_rgb(160, 96, 140),
    Color32::from_rgb(176, 120, 64),
    Color32::from_rgb(96, 128, 160),
    Color32::from_rgb(128, 108, 188),
    Color32::from_rgb(64, 128, 160),
    Color32::from_rgb(150, 128, 70),
];

impl CanvasState {
    /// Draw `preview`'s toolbar and tabs and queue what the user clicked.
    /// `enabled` is false while the page is starting or frozen.
    pub(super) fn paint_browser_chrome(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        preview: &Preview,
        layout: ChromeLayout,
        enabled: bool,
        time: f64,
    ) -> ChromeOutput {
        let mut output = ChromeOutput::default();
        let s = layout.scale;
        let corner = (8.0 * s).min(8.0);
        painter.rect_filled(
            layout.toolbar,
            egui::Rounding {
                nw: corner,
                ne: corner,
                sw: 0.0,
                se: 0.0,
            },
            ui_theme::SURFACE,
        );
        painter.rect_filled(
            layout.tabs,
            egui::Rounding {
                nw: 0.0,
                ne: 0.0,
                sw: corner,
                se: 0.0,
            },
            ui_theme::PANEL,
        );
        let border = Stroke::new(1.0, ui_theme::BORDER);
        painter.hline(layout.toolbar.x_range(), layout.toolbar.bottom(), border);
        painter.vline(layout.tabs.right(), layout.tabs.y_range(), border);
        if !shows_controls(&layout) {
            return output;
        }

        if preview.browser_loading && enabled {
            paint_loading_bar(painter, layout.toolbar, time);
            ui.ctx().request_repaint();
        }
        self.paint_toolbar(ui, painter, preview, layout, enabled, &mut output);
        self.paint_tab_strip(ui, painter, preview, layout, enabled, &mut output);
        output
    }

    fn paint_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        preview: &Preview,
        layout: ChromeLayout,
        enabled: bool,
        output: &mut ChromeOutput,
    ) {
        let s = layout.scale;
        let id = preview.id;
        let bar = layout.toolbar;
        let button = BUTTON * s;
        let slot = |left: f32| {
            Rect::from_min_size(
                Pos2::new(left, bar.center().y - button / 2.0),
                Vec2::splat(button),
            )
        };

        let mut left = bar.left() + PAD * s;
        let nav = [
            (
                egui_phosphor::regular::ARROW_LEFT,
                BrowserAction::Back,
                "Back",
                preview.browser_can_go_back,
            ),
            (
                egui_phosphor::regular::ARROW_RIGHT,
                BrowserAction::Forward,
                "Forward",
                preview.browser_can_go_forward,
            ),
            (
                egui_phosphor::regular::ARROW_CLOCKWISE,
                BrowserAction::Reload,
                "Reload",
                true,
            ),
        ];
        for (index, (icon, action, tip, available)) in nav.into_iter().enumerate() {
            let rect = slot(left);
            if icon_button(
                ui,
                painter,
                rect,
                ("browser_nav", id.0, index),
                icon,
                tip,
                enabled && available,
                None,
                s,
            ) {
                self.pending_browser_actions.push((id, action));
            }
            left = rect.right() + 2.0 * s;
        }

        let mut right = bar.right() - PAD * s;
        let close_rect = slot(right - button);
        if icon_button(
            ui,
            painter,
            close_rect,
            ("browser_close", id.0),
            egui_phosphor::regular::X,
            "Close tile",
            true,
            None,
            s,
        ) {
            output.close_tile = true;
        }
        right = close_rect.left() - 2.0 * s;
        let external_rect = slot(right - button);
        if icon_button(
            ui,
            painter,
            external_rect,
            ("browser_external", id.0),
            egui_phosphor::regular::ARROW_SQUARE_OUT,
            "Open in default browser",
            enabled,
            None,
            s,
        ) {
            self.pending_browser_actions
                .push((id, BrowserAction::OpenExternal));
        }
        right = external_rect.left() - 2.0 * s;
        let mute_rect = slot(right - button);
        let muted = preview.browser_muted;
        if icon_button(
            ui,
            painter,
            mute_rect,
            ("browser_mute", id.0),
            if muted {
                egui_phosphor::regular::SPEAKER_SLASH
            } else {
                egui_phosphor::regular::SPEAKER_HIGH
            },
            if muted { "Unmute tile" } else { "Mute tile" },
            enabled,
            muted.then_some(Color32::from_rgb(255, 150, 100)),
            s,
        ) {
            self.pending_browser_actions
                .push((id, BrowserAction::ToggleMute));
        }
        right = mute_rect.left() - PAD * s;

        let field = Rect::from_min_max(
            Pos2::new(left + PAD * s, bar.center().y - 13.0 * s),
            Pos2::new(right, bar.center().y + 13.0 * s),
        );
        if field.width() > 40.0 * s {
            self.paint_address_field(ui, painter, preview, field, enabled, s);
        }
    }

    fn paint_address_field(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        preview: &Preview,
        field: Rect,
        enabled: bool,
        s: f32,
    ) {
        let id = preview.id;
        let editing = self
            .browser_address_edit
            .as_ref()
            .is_some_and(|edit| edit.id == id);
        painter.rect_filled(field, field.height() / 2.0, FIELD_FILL);
        painter.rect_stroke(
            field,
            field.height() / 2.0,
            Stroke::new(
                1.0,
                if editing {
                    ui_theme::GOLD
                } else {
                    ui_theme::BORDER
                },
            ),
        );
        let font = egui::FontId::proportional(12.0 * s);
        let url = preview.browser_url.as_deref().unwrap_or_default();
        let secure = url.starts_with("https://");
        let icon_center = Pos2::new(field.left() + 14.0 * s, field.center().y);
        painter.text(
            icon_center,
            egui::Align2::CENTER_CENTER,
            if editing {
                egui_phosphor::regular::MAGNIFYING_GLASS
            } else if secure {
                egui_phosphor::regular::LOCK_SIMPLE
            } else {
                egui_phosphor::regular::GLOBE
            },
            egui::FontId::proportional(12.0 * s),
            ui_theme::SECONDARY,
        );
        let text_rect = Rect::from_min_max(
            Pos2::new(field.left() + 26.0 * s, field.top()),
            Pos2::new(field.right() - 12.0 * s, field.bottom()),
        );

        if editing {
            self.browser_chrome_holds_interaction |= self.interactive_browser == Some(id);
            let row_height = ui.fonts(|fonts| fonts.row_height(&font));
            let edit_rect = Rect::from_center_size(
                text_rect.center(),
                Vec2::new(text_rect.width(), row_height),
            );
            let edit = self
                .browser_address_edit
                .as_mut()
                .expect("editing implies an address edit");
            let response = ui.put(
                edit_rect,
                egui::TextEdit::singleline(&mut edit.text)
                    .frame(false)
                    .font(font)
                    .text_color(ui_theme::TEXT)
                    .margin(egui::Margin::ZERO)
                    .hint_text(if edit.new_tab {
                        "Search or enter an address for a new tab"
                    } else {
                        "Search or enter an address"
                    })
                    .desired_width(edit_rect.width()),
            );
            let right_pressed = response.hovered()
                && ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary));
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), response.id) {
                if right_pressed {
                    let (start, end) = edit.selection;
                    state.cursor.set_char_range(Some(CCursorRange::two(
                        CCursor::new(start),
                        CCursor::new(end),
                    )));
                    state.store(ui.ctx(), response.id);
                } else if response.has_focus() {
                    if let Some(range) = state.cursor.char_range() {
                        edit.selection = (range.secondary.index, range.primary.index);
                    }
                }
            }
            // Choosing from the right-click menu takes focus from the field
            // without ending the edit.
            let menu_open = response.context_menu_opened();
            if !edit.focused {
                edit.focused = true;
                response.request_focus();
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), response.id) {
                    let (start, end) = edit
                        .focus_selection
                        .take()
                        .unwrap_or((0, edit.text.chars().count()));
                    edit.selection = (start, end);
                    state.cursor.set_char_range(Some(CCursorRange::two(
                        CCursor::new(start),
                        CCursor::new(end),
                    )));
                    state.store(ui.ctx(), response.id);
                }
            } else if !response.has_focus() && !menu_open {
                let submitted = ui.input(|input| input.key_pressed(egui::Key::Enter));
                let edit = self.browser_address_edit.take().expect("still editing");
                if submitted && !edit.text.trim().is_empty() {
                    self.pending_browser_navigations.push((
                        id,
                        BrowserNavigation {
                            input: edit.text.trim().to_owned(),
                            new_tab: edit.new_tab,
                        },
                    ));
                }
                return;
            }

            let has_selection = edit.selection.0 != edit.selection.1;
            let mut command = None;
            response.context_menu(|ui| {
                crate::ui_theme::menu(ui);
                let items = [
                    ("Cut", EditCommand::Cut, has_selection),
                    ("Copy", EditCommand::Copy, has_selection),
                    ("Paste", EditCommand::Paste, true),
                    ("Paste and Go", EditCommand::PasteAndGo, true),
                ];
                for (label, item, available) in items {
                    if ui
                        .add_enabled(available, egui::Button::new(label))
                        .clicked()
                    {
                        command = Some(item);
                        ui.close_menu();
                    }
                }
                ui.separator();
                if ui.button("Select All").clicked() {
                    command = Some(EditCommand::SelectAll);
                    ui.close_menu();
                }
            });
            if let Some(command) = command {
                self.run_address_command(ui.ctx(), id, command);
            }
            return;
        }

        let response = ui.interact(
            field,
            ui.id().with(("browser_address", id.0)),
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        if response.hovered() && enabled {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }
        if response.clicked() {
            self.browser_address_edit = Some(AddressEdit::new(id, url.to_owned(), false));
        }
        let interactive = self.interactive_browser == Some(id);
        response.context_menu(|ui| {
            crate::ui_theme::menu(ui);
            self.browser_chrome_holds_interaction |= interactive;
            if ui.button("Copy Address").clicked() {
                ui.ctx().copy_text(url.to_owned());
                ui.close_menu();
            }
            if ui.button("Paste").clicked() {
                let text = pasted_address();
                let end = text.chars().count();
                let mut edit = AddressEdit::new(id, text, false);
                edit.focus_selection = Some((end, end));
                self.browser_address_edit = Some(edit);
                ui.close_menu();
            }
            if ui.button("Paste and Go").clicked() {
                self.navigate_to_clipboard(id, false);
                ui.close_menu();
            }
        });
        let job = address_job(url, &font, text_rect.width());
        let galley = painter.layout_job(job);
        painter.galley(
            Pos2::new(text_rect.left(), field.center().y - galley.size().y / 2.0),
            galley,
            ui_theme::TEXT,
        );
    }

    fn paint_tab_strip(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        preview: &Preview,
        layout: ChromeLayout,
        enabled: bool,
        output: &mut ChromeOutput,
    ) {
        let s = layout.scale;
        let id = preview.id;
        let collapsed = preview.browser_tabs_collapsed;
        let strip = layout.tabs.shrink(PAD * s);
        let row_height = TAB_HEIGHT * s;
        let gap = TAB_GAP * s;
        let painter = painter.with_clip_rect(layout.tabs.intersect(painter.clip_rect()));
        let footer = Rect::from_min_max(
            Pos2::new(strip.left(), strip.bottom() - row_height),
            strip.max,
        );
        let list_bottom = footer.top() - gap;

        let fallback;
        let (tabs, active_tab) = if preview.browser_tabs.is_empty() {
            fallback = [BrowserTab {
                url: preview.browser_url.clone().unwrap_or_default(),
                title: preview.title.clone(),
                playing_audio: preview.browser_playing_audio,
            }];
            (&fallback[..], 0)
        } else {
            (
                &preview.browser_tabs[..],
                preview
                    .browser_active_tab
                    .min(preview.browser_tabs.len() - 1),
            )
        };
        let font = egui::FontId::proportional(12.0 * s);
        let can_close = tabs.len() > 1;
        let mut top = strip.top();
        for (index, tab) in tabs.iter().enumerate() {
            let row = Rect::from_min_size(
                Pos2::new(strip.left(), top),
                Vec2::new(strip.width(), row_height),
            );
            if row.bottom() > list_bottom {
                break;
            }
            top = row.bottom() + gap;
            let active = index == active_tab;
            let response = ui.interact(
                row,
                ui.id().with(("browser_tab", id.0, index)),
                if enabled {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            );
            let hovered = response.hovered();
            if active {
                painter.rect_filled(row, 6.0 * s, ui_theme::GOLD_DARK);
                painter.rect_filled(
                    Rect::from_min_size(
                        row.left_top() + Vec2::new(0.0, 6.0 * s),
                        Vec2::new(2.5 * s, row.height() - 12.0 * s),
                    ),
                    1.5 * s,
                    ui_theme::GOLD,
                );
            } else if hovered {
                painter.rect_filled(row, 6.0 * s, ui_theme::HOVER);
            }
            let badge = Rect::from_center_size(
                if collapsed {
                    row.center()
                } else {
                    Pos2::new(row.left() + 8.0 * s + BADGE * s / 2.0, row.center().y)
                },
                Vec2::splat(BADGE * s),
            );
            paint_site_badge(&painter, badge, &tab.url, s);

            let mut closed = false;
            if !collapsed {
                let trailing = 22.0 * s;
                let label_rect = Rect::from_min_max(
                    Pos2::new(badge.right() + 8.0 * s, row.top()),
                    Pos2::new(row.right() - trailing, row.bottom()),
                );
                let color = if active {
                    ui_theme::TEXT
                } else {
                    ui_theme::SECONDARY
                };
                let galley =
                    painter.layout_job(single_line(tab.label(), &font, color, label_rect.width()));
                painter.galley(
                    Pos2::new(label_rect.left(), row.center().y - galley.size().y / 2.0),
                    galley,
                    color,
                );
                let trailing_rect = Rect::from_center_size(
                    Pos2::new(row.right() - trailing / 2.0, row.center().y),
                    Vec2::splat(18.0 * s),
                );
                if can_close && enabled && hovered {
                    closed = icon_button(
                        ui,
                        &painter,
                        trailing_rect,
                        ("browser_tab_close", id.0, index),
                        egui_phosphor::regular::X,
                        "Close tab",
                        true,
                        None,
                        s * 0.8,
                    );
                } else if tab.playing_audio {
                    painter.text(
                        trailing_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        egui_phosphor::regular::SPEAKER_HIGH,
                        egui::FontId::proportional(12.0 * s),
                        ui_theme::SECONDARY,
                    );
                }
            } else if tab.playing_audio {
                painter.circle_filled(
                    badge.right_top() + Vec2::new(-1.0, 1.0) * s,
                    3.0 * s,
                    ui_theme::GOLD,
                );
            }

            let response = if collapsed {
                response.on_hover_text(tab.label())
            } else {
                response
            };
            let interactive = self.interactive_browser == Some(id);
            response.context_menu(|ui| {
                crate::ui_theme::menu(ui);
                self.browser_chrome_holds_interaction |= interactive;
                if ui.button("Copy Address").clicked() {
                    ui.ctx().copy_text(tab.url.clone());
                    ui.close_menu();
                }
                if ui.button("New Tab").clicked() {
                    self.browser_address_edit = Some(AddressEdit::new(id, String::new(), true));
                    ui.close_menu();
                }
                ui.separator();
                if ui
                    .add_enabled(can_close, egui::Button::new("Close Tab"))
                    .clicked()
                {
                    self.pending_browser_actions
                        .push((id, BrowserAction::CloseTab(index)));
                    ui.close_menu();
                }
            });
            if closed || (can_close && enabled && response.middle_clicked()) {
                self.pending_browser_actions
                    .push((id, BrowserAction::CloseTab(index)));
            } else if response.clicked() && !active {
                self.pending_browser_actions
                    .push((id, BrowserAction::SelectTab(index)));
            }
        }

        let new_tab_row = Rect::from_min_size(
            Pos2::new(strip.left(), top),
            Vec2::new(strip.width(), row_height),
        );
        if new_tab_row.bottom() <= list_bottom {
            let response = ui.interact(
                new_tab_row,
                ui.id().with(("browser_new_tab", id.0)),
                if enabled {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            );
            if response.hovered() && enabled {
                painter.rect_filled(new_tab_row, 6.0 * s, ui_theme::HOVER);
            }
            let icon_center = if collapsed {
                new_tab_row.center()
            } else {
                Pos2::new(
                    new_tab_row.left() + 8.0 * s + BADGE * s / 2.0,
                    new_tab_row.center().y,
                )
            };
            let color = if enabled {
                ui_theme::SECONDARY
            } else {
                DISABLED
            };
            painter.text(
                icon_center,
                egui::Align2::CENTER_CENTER,
                egui_phosphor::regular::PLUS,
                egui::FontId::proportional(14.0 * s),
                color,
            );
            if !collapsed {
                painter.text(
                    Pos2::new(icon_center.x + BADGE * s / 2.0 + 8.0 * s, icon_center.y),
                    egui::Align2::LEFT_CENTER,
                    "New tab",
                    font.clone(),
                    color,
                );
            }
            if response.on_hover_text("New tab").clicked() {
                self.browser_address_edit = Some(AddressEdit::new(id, String::new(), true));
            }
        }

        let toggle = Rect::from_min_size(footer.min, Vec2::splat(row_height));
        if icon_button(
            ui,
            &painter,
            toggle,
            ("browser_tabs_toggle", id.0),
            egui_phosphor::regular::SIDEBAR_SIMPLE,
            if collapsed {
                "Expand tabs"
            } else {
                "Collapse tabs"
            },
            true,
            None,
            s,
        ) {
            output.toggle_tabs_collapsed = true;
        }
    }
}

/// Whether the frame is large enough on screen to draw its buttons, address
/// and tabs; below this only its shapes are drawn.
pub(super) fn shows_controls(layout: &ChromeLayout) -> bool {
    layout.scale >= MIN_DETAIL_SCALE
}

impl CanvasState {
    /// Apply an address bar menu choice to the text being typed.
    fn run_address_command(&mut self, ctx: &egui::Context, id: PreviewId, command: EditCommand) {
        let Some(edit) = self.browser_address_edit.as_mut() else {
            return;
        };
        let (start, end) = ordered(edit.selection);
        match command {
            EditCommand::Cut | EditCommand::Copy => {
                ctx.copy_text(char_slice(&edit.text, start, end).to_owned());
                if command == EditCommand::Cut {
                    let cursor = replace_chars(&mut edit.text, start, end, "");
                    edit.focus_selection = Some((cursor, cursor));
                } else {
                    edit.focus_selection = Some(edit.selection);
                }
            }
            EditCommand::Paste => {
                let cursor = replace_chars(&mut edit.text, start, end, &pasted_address());
                edit.focus_selection = Some((cursor, cursor));
            }
            EditCommand::PasteAndGo => {
                let new_tab = edit.new_tab;
                self.browser_address_edit = None;
                self.navigate_to_clipboard(id, new_tab);
                return;
            }
            EditCommand::SelectAll => {
                edit.focus_selection = Some((0, edit.text.chars().count()));
            }
        }
        // Give the field its focus back next frame.
        edit.focused = false;
    }

    /// Open the address on the clipboard, or search for its text.
    fn navigate_to_clipboard(&mut self, id: PreviewId, new_tab: bool) {
        let input = pasted_address();
        if !input.is_empty() {
            self.pending_browser_navigations
                .push((id, BrowserNavigation { input, new_tab }));
        }
    }
}

/// Clipboard text as one trimmed line, or empty when there is none.
fn pasted_address() -> String {
    #[cfg(windows)]
    let text = crate::clipboard::text().unwrap_or_default();
    #[cfg(not(windows))]
    let text = String::new();
    single_line_text(&text)
}

/// Text pasted into the single-line address bar: line breaks become spaces.
fn single_line_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ordered((a, b): (usize, usize)) -> (usize, usize) {
    (a.min(b), a.max(b))
}

fn char_byte(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(byte, _)| byte)
}

fn char_slice(text: &str, start: usize, end: usize) -> &str {
    &text[char_byte(text, start)..char_byte(text, end)]
}

/// Replace characters `start..end` with `insert`; returns the character
/// index just after the inserted text.
fn replace_chars(text: &mut String, start: usize, end: usize, insert: &str) -> usize {
    let range = char_byte(text, start)..char_byte(text, end);
    text.replace_range(range, insert);
    start + insert.chars().count()
}

/// A square icon button in the frame. Returns true when clicked.
#[allow(clippy::too_many_arguments)]
fn icon_button(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: Rect,
    id_salt: impl std::hash::Hash,
    icon: &str,
    tip: &str,
    enabled: bool,
    color: Option<Color32>,
    s: f32,
) -> bool {
    let response = ui.interact(
        rect,
        ui.id().with(id_salt),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hovered = enabled && response.hovered();
    if hovered {
        let alpha = if response.is_pointer_button_down_on() {
            60
        } else {
            30
        };
        painter.rect_filled(
            rect,
            6.0 * s,
            Color32::from_rgba_unmultiplied(255, 255, 255, alpha),
        );
    }
    let icon_color = if !enabled {
        DISABLED
    } else if hovered {
        ui_theme::TEXT
    } else {
        color.unwrap_or(ui_theme::SECONDARY)
    };
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(15.0 * s),
        icon_color,
    );
    let clicked = enabled && response.clicked();
    response.on_hover_text(tip);
    clicked
}

/// A thin gold sweep along the toolbar's bottom edge while a page loads.
fn paint_loading_bar(painter: &egui::Painter, toolbar: Rect, time: f64) {
    let width = toolbar.width();
    let phase = (time * 0.9).fract() as f32;
    let start = toolbar.left() + (phase * 1.4 - 0.4) * width;
    let segment = Rect::from_min_max(
        Pos2::new(start.max(toolbar.left()), toolbar.bottom() - 2.0),
        Pos2::new((start + width * 0.4).min(toolbar.right()), toolbar.bottom()),
    );
    if segment.width() > 0.0 {
        painter.rect_filled(segment, 1.0, ui_theme::GOLD);
    }
}

/// The address without its scheme: the site in full color, the rest dimmed.
fn address_job(url: &str, font: &egui::FontId, max_width: f32) -> LayoutJob {
    let shown = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let split = shown.find(['/', '?']).unwrap_or(shown.len());
    let (site, rest) = shown.split_at(split);
    let rest = if rest == "/" { "" } else { rest };
    let mut job = LayoutJob::default();
    job.append(site, 0.0, TextFormat::simple(font.clone(), ui_theme::TEXT));
    job.append(
        rest,
        0.0,
        TextFormat::simple(font.clone(), ui_theme::SECONDARY),
    );
    job.wrap = TextWrapping::truncate_at_width(max_width.max(1.0));
    job
}

fn single_line(text: &str, font: &egui::FontId, color: Color32, max_width: f32) -> LayoutJob {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font.clone(), color);
    job.wrap = TextWrapping::truncate_at_width(max_width.max(1.0));
    job
}

/// The host part of `url`, without a leading `www.`.
fn site_of(url: &str) -> &str {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = after_scheme
        .split(['/', '?', '#', ':'])
        .next()
        .unwrap_or_default();
    host.strip_prefix("www.").unwrap_or(host)
}

/// The name a site is known by: `wikipedia` for `en.wikipedia.org`, `bbc`
/// for `www.bbc.co.uk`.
fn site_name(host: &str) -> &str {
    let labels: Vec<&str> = host.split('.').filter(|label| !label.is_empty()).collect();
    match labels.len() {
        0 => host,
        1 => labels[0],
        count => {
            let name = labels[count - 2];
            // Second-level country domains such as co.uk or com.au.
            if count >= 3 && name.len() <= 3 && labels[count - 1].len() == 2 {
                labels[count - 3]
            } else {
                name
            }
        }
    }
}

/// A colored square with the site's initial, standing in for its icon.
fn paint_site_badge(painter: &egui::Painter, rect: Rect, url: &str, s: f32) {
    let site = site_name(site_of(url));
    let hash = site.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    let color = BADGE_COLORS[hash as usize % BADGE_COLORS.len()];
    painter.rect_filled(rect, 4.0 * s, color);
    let initial = site
        .chars()
        .find(|character| character.is_alphanumeric())
        .map(|character| character.to_uppercase().to_string())
        .unwrap_or_else(|| "•".to_owned());
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(10.5 * s),
        Color32::from_rgb(240, 240, 244),
    );
}

#[cfg(test)]
mod tests {
    use super::{
        address_job, char_slice, ordered, replace_chars, single_line_text, site_name, site_of,
    };
    use eframe::egui;

    #[test]
    fn menu_edits_replace_the_selection_by_character() {
        let mut text = "héllo world".to_owned();
        assert_eq!(char_slice(&text, 1, 5), "éllo");
        assert_eq!(replace_chars(&mut text, 6, 11, "there"), 11);
        assert_eq!(text, "héllo there");
        assert_eq!(replace_chars(&mut text, 0, 6, ""), 0);
        assert_eq!(text, "there");
        assert_eq!(ordered((4, 1)), (1, 4));
    }

    #[test]
    fn pasted_text_becomes_one_line() {
        assert_eq!(
            single_line_text("  https://example.com/\r\n  path \n"),
            "https://example.com/ path"
        );
    }

    #[test]
    fn site_drops_scheme_path_port_and_www() {
        assert_eq!(site_of("https://www.youtube.com/watch?v=1"), "youtube.com");
        assert_eq!(site_of("http://localhost:8080/"), "localhost");
        assert_eq!(site_of("example.org"), "example.org");
    }

    #[test]
    fn badges_name_the_site_rather_than_its_subdomain() {
        assert_eq!(site_name("en.wikipedia.org"), "wikipedia");
        assert_eq!(site_name("www.bbc.co.uk"), "bbc");
        assert_eq!(site_name("github.com"), "github");
        assert_eq!(site_name("localhost"), "localhost");
    }

    #[test]
    fn address_shows_the_site_apart_from_its_path() {
        let font = egui::FontId::proportional(12.0);
        let job = address_job("https://twitch.tv/somechannel?x=1", &font, 200.0);
        assert_eq!(job.text, "twitch.tv/somechannel?x=1");
        assert_eq!(job.sections.len(), 2);
        assert_eq!(job.sections[0].byte_range, 0.."twitch.tv".len());
        let bare = address_job("https://example.com/", &font, 200.0);
        assert_eq!(bare.text, "example.com");
    }
}
