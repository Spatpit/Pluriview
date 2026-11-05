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

/// A tab being dragged in a browser tile's strip.
#[derive(Clone, Copy)]
pub(super) struct TabDrag {
    id: PreviewId,
    from: usize,
}

/// How far a tile's tab list is scrolled, in tile units, and which tab it
/// last kept in view.
#[derive(Clone)]
pub(super) struct TabScroll {
    offset: f32,
    active: usize,
    count: usize,
}

/// A tab dropped outside its tile, resolved once every tile is drawn.
#[derive(Clone, Copy)]
pub(super) struct TabDrop {
    drag: TabDrag,
    pointer: Pos2,
}

/// A tab dragged onto another browser tile to join its tabs, consumed by the
/// app.
#[derive(Clone, Copy, Debug)]
pub struct TabTransfer {
    pub from: PreviewId,
    pub index: usize,
    pub to: PreviewId,
    /// Where it goes among `to`'s tabs.
    pub slot: usize,
}

/// Another browser tile a dragged tab would join.
struct TabDropTarget {
    id: PreviewId,
    slot: usize,
    tile: Rect,
    /// The insertion line in its tab strip, when it shows one: x range, y
    /// and the strip's scale.
    line: Option<(egui::Rangef, f32, f32)>,
}

/// A tab dragged out of its tile (or moved with its menu) to open as a new
/// tile, consumed by the app.
#[derive(Clone, Copy, Debug)]
pub struct TabDetach {
    pub id: PreviewId,
    pub index: usize,
    /// Where it was dropped on the canvas, or None to place it beside.
    pub position: Option<Pos2>,
}

/// The scrolling list of tabs and the New tab row, above the collapse button.
struct TabList {
    area: Rect,
    row_height: f32,
    gap: f32,
    pitch: f32,
    /// How far the list can scroll, in screen points.
    max_scroll: f32,
    scale: f32,
}

impl TabList {
    fn new(layout: &ChromeLayout, tab_count: usize) -> Self {
        let s = layout.scale;
        let strip = layout.tabs.shrink(PAD * s);
        let row_height = TAB_HEIGHT * s;
        let gap = TAB_GAP * s;
        let bottom = (strip.bottom() - row_height - gap).max(strip.top());
        let area = Rect::from_min_max(strip.min, Pos2::new(strip.right(), bottom));
        let pitch = row_height + gap;
        let content = (tab_count + 1) as f32 * pitch - gap;
        Self {
            area,
            row_height,
            gap,
            pitch,
            max_scroll: (content - area.height()).max(0.0),
            scale: s,
        }
    }

    /// Row `index` (the New tab row after the tabs) at scroll `scroll`.
    fn row(&self, index: usize, scroll: f32) -> Rect {
        Rect::from_min_size(
            Pos2::new(
                self.area.left(),
                self.area.top() + index as f32 * self.pitch - scroll,
            ),
            Vec2::new(self.area.width(), self.row_height),
        )
    }

    /// The gap (0..=count) a tab dropped at height `y` goes into.
    fn slot_at(&self, y: f32, scroll: f32, count: usize) -> usize {
        let position = (y - self.area.top() + scroll) / self.pitch;
        ((position + 0.5).floor().max(0.0) as usize).min(count)
    }

    fn paint_scrollbar(&self, painter: &egui::Painter, scroll: f32, s: f32) {
        if self.max_scroll <= 0.0 {
            return;
        }
        let track = self.area.height();
        let thumb = (track * track / (track + self.max_scroll)).max(12.0 * s);
        let top = self.area.top() + (track - thumb) * (scroll / self.max_scroll);
        let x = self.area.right() + PAD * s / 2.0;
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(x - 1.5 * s, top),
                Pos2::new(x + 1.5 * s, top + thumb),
            ),
            1.5 * s,
            Color32::from_gray(96),
        );
    }
}

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
            egui::epaint::CornerRadiusF32 {
                nw: corner,
                ne: corner,
                sw: 0.0,
                se: 0.0,
            },
            ui_theme::SURFACE,
        );
        painter.rect_filled(
            layout.tabs,
            egui::epaint::CornerRadiusF32 {
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
            // A thin sweeping bar needs no more than 30 frames a second.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
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
            egui::StrokeKind::Middle,
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
            let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
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
                    .frame(egui::Frame::NONE)
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
                        edit.selection = (range.secondary.index.0, range.primary.index.0);
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
            crate::ui_theme::context_menu(&response, |ui| {
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
                        ui.close_kind(egui::UiKind::Menu);
                    }
                }
                ui.separator();
                if ui.button("Select All").clicked() {
                    command = Some(EditCommand::SelectAll);
                    ui.close_kind(egui::UiKind::Menu);
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
        crate::ui_theme::context_menu(&response, |ui| {
            crate::ui_theme::menu(ui);
            self.browser_chrome_holds_interaction |= interactive;
            if ui.button("Copy Address").clicked() {
                ui.ctx().copy_text(url.to_owned());
                ui.close_kind(egui::UiKind::Menu);
            }
            if ui.button("Paste").clicked() {
                let text = pasted_address();
                let end = text.chars().count();
                let mut edit = AddressEdit::new(id, text, false);
                edit.focus_selection = Some((end, end));
                self.browser_address_edit = Some(edit);
                ui.close_kind(egui::UiKind::Menu);
            }
            if ui.button("Paste and Go").clicked() {
                self.navigate_to_clipboard(id, false);
                ui.close_kind(egui::UiKind::Menu);
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
        let painter = painter.with_clip_rect(layout.tabs.intersect(painter.clip_rect()));
        let footer = Rect::from_min_max(
            Pos2::new(strip.left(), strip.bottom() - row_height),
            strip.max,
        );

        let fallback;
        let (tabs, active_tab) = if preview.browser_tabs.is_empty() {
            fallback = [BrowserTab {
                url: preview.browser_url.clone().unwrap_or_default(),
                title: preview.title.clone(),
                playing_audio: preview.browser_playing_audio,
                ..BrowserTab::default()
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
        let list = TabList::new(&layout, tabs.len());
        let scroll = self.tab_list_scroll(id, &list, active_tab, tabs.len());
        let list_painter = painter.with_clip_rect(list.area.intersect(painter.clip_rect()));
        let font = egui::FontId::proportional(12.0 * s);
        let can_close = tabs.len() > 1;
        let dragging = self
            .browser_tab_drag
            .filter(|drag| drag.id == id)
            .map(|drag| drag.from);
        for (index, tab) in tabs.iter().enumerate() {
            let row = list.row(index, scroll);
            let hit = row.intersect(list.area);
            if hit.height() <= 0.0 {
                continue;
            }
            let active = index == active_tab;
            let response = ui.interact(
                hit,
                ui.id().with(("browser_tab", id.0, index)),
                if enabled {
                    Sense::click_and_drag()
                } else {
                    Sense::hover()
                },
            );
            if response.drag_started() {
                self.browser_tab_drag = Some(TabDrag { id, from: index });
            }
            let hovered = response.hovered() && dragging.is_none();
            if active {
                list_painter.rect_filled(row, 6.0 * s, ui_theme::GOLD_DARK);
                list_painter.rect_filled(
                    Rect::from_min_size(
                        row.left_top() + Vec2::new(0.0, 6.0 * s),
                        Vec2::new(2.5 * s, row.height() - 12.0 * s),
                    ),
                    1.5 * s,
                    ui_theme::GOLD,
                );
            } else if hovered || dragging == Some(index) {
                list_painter.rect_filled(row, 6.0 * s, ui_theme::HOVER);
            }
            // Sleeping tabs fade until they are shown again.
            let faded = tab.sleeping && !active;
            let badge = Rect::from_center_size(
                if collapsed {
                    row.center()
                } else {
                    Pos2::new(row.left() + 8.0 * s + BADGE * s / 2.0, row.center().y)
                },
                Vec2::splat(BADGE * s),
            );
            self.paint_tab_icon(ui.ctx(), &list_painter, badge, tab, faded, s);

            let mut closed = false;
            if !collapsed {
                let trailing = 22.0 * s;
                let label_rect = Rect::from_min_max(
                    Pos2::new(badge.right() + 8.0 * s, row.top()),
                    Pos2::new(row.right() - trailing, row.bottom()),
                );
                let color = if active {
                    ui_theme::TEXT
                } else if faded {
                    DISABLED
                } else {
                    ui_theme::SECONDARY
                };
                let galley =
                    painter.layout_job(single_line(tab.label(), &font, color, label_rect.width()));
                list_painter.galley(
                    Pos2::new(label_rect.left(), row.center().y - galley.size().y / 2.0),
                    galley,
                    color,
                );
                let trailing_rect = Rect::from_center_size(
                    Pos2::new(row.right() - trailing / 2.0, row.center().y),
                    Vec2::splat(18.0 * s),
                );
                if can_close && enabled && hovered && list.area.contains_rect(trailing_rect) {
                    closed = icon_button(
                        ui,
                        &list_painter,
                        trailing_rect,
                        ("browser_tab_close", id.0, index),
                        egui_phosphor::regular::X,
                        "Close tab",
                        true,
                        None,
                        s * 0.8,
                    );
                } else if tab.playing_audio {
                    list_painter.text(
                        trailing_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        egui_phosphor::regular::SPEAKER_HIGH,
                        egui::FontId::proportional(12.0 * s),
                        ui_theme::SECONDARY,
                    );
                } else if faded {
                    list_painter.text(
                        trailing_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        egui_phosphor::regular::MOON,
                        egui::FontId::proportional(11.0 * s),
                        DISABLED,
                    );
                }
            } else if tab.playing_audio {
                list_painter.circle_filled(
                    badge.right_top() + Vec2::new(-1.0, 1.0) * s,
                    3.0 * s,
                    ui_theme::GOLD,
                );
            }

            let response = if collapsed || faded {
                response.on_hover_text(if faded {
                    format!("{}\nSleeping. Opens again when you show it.", tab.label())
                } else {
                    tab.label().to_owned()
                })
            } else {
                response
            };
            let interactive = self.interactive_browser == Some(id);
            crate::ui_theme::context_menu(&response, |ui| {
                crate::ui_theme::menu(ui);
                self.browser_chrome_holds_interaction |= interactive;
                if ui.button("Copy Address").clicked() {
                    ui.ctx().copy_text(tab.url.clone());
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button("New Tab").clicked() {
                    self.browser_address_edit = Some(AddressEdit::new(id, String::new(), true));
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui
                    .add_enabled(can_close, egui::Button::new("Move to New Tile"))
                    .clicked()
                {
                    self.detach_tab(id, index, None);
                    ui.close_kind(egui::UiKind::Menu);
                }
                ui.separator();
                if ui
                    .add_enabled(can_close, egui::Button::new("Close Tab"))
                    .clicked()
                {
                    self.pending_browser_actions
                        .push((id, BrowserAction::CloseTab(index)));
                    ui.close_kind(egui::UiKind::Menu);
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

        let new_tab_row = list.row(tabs.len(), scroll);
        let new_tab_hit = new_tab_row.intersect(list.area);
        if new_tab_hit.height() > 0.0 {
            let response = ui.interact(
                new_tab_hit,
                ui.id().with(("browser_new_tab", id.0)),
                if enabled {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            );
            if response.hovered() && enabled && dragging.is_none() {
                list_painter.rect_filled(new_tab_row, 6.0 * s, ui_theme::HOVER);
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
            list_painter.text(
                icon_center,
                egui::Align2::CENTER_CENTER,
                egui_phosphor::regular::PLUS,
                egui::FontId::proportional(14.0 * s),
                color,
            );
            if !collapsed {
                list_painter.text(
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
        list.paint_scrollbar(&painter, scroll, s);

        if let Some(from) = dragging {
            self.drag_tab(ui, &list_painter, &layout, &list, scroll, from, tabs);
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

    /// The tab list's scroll offset in screen points, kept in range and
    /// moved so a newly shown or opened tab is in view.
    fn tab_list_scroll(
        &mut self,
        id: PreviewId,
        list: &TabList,
        active_tab: usize,
        tab_count: usize,
    ) -> f32 {
        let s = list.scale;
        let state = self.browser_tab_scroll.entry(id).or_insert(TabScroll {
            offset: 0.0,
            active: usize::MAX,
            count: 0,
        });
        if state.active != active_tab || state.count != tab_count {
            state.active = active_tab;
            state.count = tab_count;
            let top = active_tab as f32 * list.pitch / s;
            let bottom = top + list.row_height / s;
            let view = list.area.height() / s;
            if top < state.offset {
                state.offset = top;
            } else if bottom > state.offset + view {
                state.offset = bottom - view;
            }
        }
        state.offset = state.offset.clamp(0.0, list.max_scroll / s);
        state.offset * s
    }

    /// Scroll the tab strip under `pointer` by a wheel `delta` (points).
    /// Returns false when there is no overflowing tab strip there, so the
    /// wheel zooms the canvas as usual.
    pub(super) fn scroll_tab_strip_at(
        &mut self,
        pointer: Pos2,
        delta: f32,
        canvas_rect: Rect,
        preview_manager: &PreviewManager,
    ) -> bool {
        let Some(preview) = self
            .left_click_preview_at_screen(pointer, canvas_rect, preview_manager)
            .and_then(|id| preview_manager.get(id))
        else {
            return false;
        };
        let rect = self.preview_screen_rect(preview, canvas_rect);
        let Some(layout) = chrome_geometry::layout_for(preview, rect) else {
            return false;
        };
        let list = TabList::new(&layout, preview.browser_tabs.len().max(1));
        if !shows_controls(&layout) || list.max_scroll <= 0.0 || !list.area.contains(pointer) {
            return false;
        }
        if let Some(state) = self.browser_tab_scroll.get_mut(&preview.id) {
            state.offset =
                (state.offset - delta / layout.scale).clamp(0.0, list.max_scroll / layout.scale);
        }
        true
    }

    /// Whether `pointer` is on the tab list of `preview` drawn at `rect`.
    pub(super) fn over_tab_list(&self, preview: &Preview, rect: Rect, pointer: Pos2) -> bool {
        chrome_geometry::layout_for(preview, rect).is_some_and(|layout| {
            shows_controls(&layout)
                && TabList::new(&layout, preview.browser_tabs.len().max(1))
                    .area
                    .contains(pointer)
        })
    }

    /// Follow a tab being dragged within its own tile and show where it will
    /// land in the strip. Outside the tile, [`Self::follow_tab_drag`] takes
    /// over.
    #[allow(clippy::too_many_arguments)]
    fn drag_tab(
        &mut self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        layout: &ChromeLayout,
        list: &TabList,
        scroll: f32,
        from: usize,
        tabs: &[BrowserTab],
    ) {
        let s = layout.scale;
        let Some(drag) = self.browser_tab_drag else {
            return;
        };
        let Some(pointer) = ui.ctx().pointer_latest_pos() else {
            self.browser_tab_drag = None;
            return;
        };
        let tile = layout.toolbar.union(layout.page);
        let inside = tile.contains(pointer);
        let slot = list.slot_at(pointer.y, scroll, tabs.len());
        if !ui.input(|input| input.pointer.primary_down()) {
            self.browser_tab_drag = None;
            // A drop counts only on the frame the button comes up; a drag
            // left over from a tile that went off screen just ends.
            if !ui.input(|input| input.pointer.primary_released()) {
                return;
            }
            if inside {
                let to = if slot > from { slot - 1 } else { slot };
                if to != from && to < tabs.len() {
                    self.pending_browser_actions
                        .push((drag.id, BrowserAction::MoveTab { from, to }));
                }
            } else {
                self.browser_tab_drop = Some(TabDrop { drag, pointer });
            }
            return;
        }

        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        ui.ctx().request_repaint();
        self.browser_chrome_holds_interaction |= self.interactive_browser == Some(drag.id);
        if inside {
            let y = list.area.top() + slot as f32 * list.pitch - scroll - list.gap / 2.0;
            painter.hline(
                list.area.x_range(),
                y.clamp(list.area.top() + 1.0, list.area.bottom() - 1.0),
                Stroke::new(2.0 * s.max(0.5), ui_theme::GOLD),
            );
            // Near the ends of an overflowing list, scroll to reach the rest.
            let edge = list.row_height / 2.0;
            let step = if pointer.y < list.area.top() + edge {
                -6.0
            } else if pointer.y > list.area.bottom() - edge {
                6.0
            } else {
                0.0
            };
            if let Some(state) = self.browser_tab_scroll.get_mut(&drag.id) {
                state.offset = (state.offset + step).clamp(0.0, list.max_scroll / s);
            }
        }
    }

    /// After every tile is drawn: resolve a tab dropped outside its tile,
    /// or show where a tab being dragged there would go. On another browser
    /// tile it joins that tile's tabs, elsewhere it opens as a new tile.
    pub(super) fn follow_tab_drag(
        &mut self,
        ctx: &egui::Context,
        canvas_rect: Rect,
        preview_manager: &PreviewManager,
    ) {
        if let Some(drop) = self.browser_tab_drop.take() {
            let drag = drop.drag;
            let tab_count = preview_manager
                .get(drag.id)
                .map_or(0, |preview| preview.browser_tabs.len().max(1));
            match self.tab_drop_target(drag, drop.pointer, canvas_rect, preview_manager) {
                Some(target) => self.pending_tab_transfers.push(TabTransfer {
                    from: drag.id,
                    index: drag.from,
                    to: target.id,
                    slot: target.slot,
                }),
                // The last tab only moves to another tile; alone it would
                // just be this tile again.
                None if tab_count > 1 => self.detach_tab(drag.id, drag.from, Some(drop.pointer)),
                None => {}
            }
            return;
        }
        let Some(drag) = self.browser_tab_drag else {
            return;
        };
        let (Some(pointer), Some(source)) =
            (ctx.pointer_latest_pos(), preview_manager.get(drag.id))
        else {
            return;
        };
        let source_rect = self.preview_screen_rect(source, canvas_rect);
        let source_tile = chrome_geometry::layout_for(source, source_rect)
            .map_or(source_rect, |layout| layout.toolbar.union(layout.page));
        if source_tile.contains(pointer) {
            return;
        }
        let fallback;
        let tabs = if source.browser_tabs.is_empty() {
            fallback = [BrowserTab {
                url: source.browser_url.clone().unwrap_or_default(),
                title: source.title.clone(),
                ..BrowserTab::default()
            }];
            &fallback[..]
        } else {
            &source.browser_tabs[..]
        };
        let Some(tab) = tabs.get(drag.from) else {
            return;
        };
        let target = self.tab_drop_target(drag, pointer, canvas_rect, preview_manager);
        let ghost_painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("browser_tab_drag_ghost"),
        ));
        if let Some(target) = &target {
            ghost_painter.rect_stroke(
                target.tile,
                6.0,
                Stroke::new(2.0, ui_theme::GOLD),
                egui::StrokeKind::Middle,
            );
            if let Some((x, y, s)) = target.line {
                ghost_painter.hline(x, y, Stroke::new(2.0 * s.max(0.5), ui_theme::GOLD));
            }
        }
        let hint = match (&target, tabs.len() > 1) {
            (Some(_), _) => "Drop to add to this tile's tabs",
            (None, true) => "Drop to open as a new tile",
            (None, false) => "Drop on a browser tile to move it there",
        };
        let ghost = Rect::from_min_size(pointer + Vec2::new(14.0, 10.0), Vec2::new(200.0, 52.0));
        ghost_painter.rect_filled(ghost, 8.0, ui_theme::SURFACE);
        ghost_painter.rect_stroke(
            ghost,
            8.0,
            Stroke::new(1.5, ui_theme::GOLD),
            egui::StrokeKind::Middle,
        );
        let badge = Rect::from_center_size(
            Pos2::new(ghost.left() + 20.0, ghost.top() + 18.0),
            Vec2::splat(BADGE),
        );
        self.paint_tab_icon(ctx, &ghost_painter, badge, tab, false, 1.0);
        let font = egui::FontId::proportional(12.0);
        let galley = ghost_painter.layout_job(single_line(
            tab.label(),
            &font,
            ui_theme::TEXT,
            ghost.width() - 44.0,
        ));
        ghost_painter.galley(
            Pos2::new(
                badge.right() + 8.0,
                badge.center().y - galley.size().y / 2.0,
            ),
            galley,
            ui_theme::TEXT,
        );
        ghost_painter.text(
            Pos2::new(ghost.left() + 12.0, ghost.bottom() - 12.0),
            egui::Align2::LEFT_CENTER,
            hint,
            egui::FontId::proportional(11.0),
            ui_theme::SECONDARY,
        );
    }

    /// The browser tile under `pointer` that a tab dragged out of its own
    /// tile would join, and where among its tabs.
    fn tab_drop_target(
        &self,
        drag: TabDrag,
        pointer: Pos2,
        canvas_rect: Rect,
        preview_manager: &PreviewManager,
    ) -> Option<TabDropTarget> {
        let id = self
            .left_click_preview_at_screen(pointer, canvas_rect, preview_manager)
            .filter(|id| *id != drag.id)?;
        let preview = preview_manager.get(id).filter(|preview| {
            preview.is_browser()
                && preview.removing.is_none()
                && !preview.manually_frozen
                && preview.picture_in_picture.is_none()
                && preview.browser_status == BrowserTileStatus::Ready
        })?;
        let tile = self.preview_screen_rect(preview, canvas_rect);
        let count = preview.browser_tabs.len().max(1);
        let mut target = TabDropTarget {
            id,
            // Anywhere else on the tile, it goes after the last tab.
            slot: count,
            tile,
            line: None,
        };
        if let Some(layout) = chrome_geometry::layout_for(preview, tile).filter(shows_controls) {
            let list = TabList::new(&layout, count);
            let scroll = self
                .browser_tab_scroll
                .get(&id)
                .map_or(0.0, |state| state.offset * layout.scale)
                .clamp(0.0, list.max_scroll);
            if list.area.contains(pointer) {
                target.slot = list.slot_at(pointer.y, scroll, count);
            }
            let y = list.area.top() + target.slot as f32 * list.pitch - scroll - list.gap / 2.0;
            target.line = Some((
                list.area.x_range(),
                y.clamp(list.area.top() + 1.0, list.area.bottom() - 1.0),
                layout.scale,
            ));
        }
        Some(target)
    }

    /// Queue moving tab `index` of `id` to a new tile, dropped at `pointer`
    /// (screen points) or beside the tile.
    fn detach_tab(&mut self, id: PreviewId, index: usize, pointer: Option<Pos2>) {
        let position = pointer
            .zip(self.last_screen_rect)
            .map(|(pointer, canvas_rect)| self.screen_to_canvas(pointer, canvas_rect));
        self.pending_tab_detaches.push(TabDetach {
            id,
            index,
            position,
        });
    }

    /// A tab's site icon, or a lettered badge while it has none.
    fn paint_tab_icon(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        rect: Rect,
        tab: &BrowserTab,
        faded: bool,
        s: f32,
    ) {
        let alpha = if faded { 0.45 } else { 1.0 };
        let Some(favicon) = &tab.favicon else {
            paint_site_badge(painter, rect, &tab.url, alpha, s);
            return;
        };
        let texture = self
            .favicon_textures
            .entry(favicon.uri.clone())
            .or_insert_with(|| {
                ctx.load_texture(
                    format!("favicon:{}", favicon.uri),
                    (*favicon.image).clone(),
                    egui::TextureOptions::LINEAR,
                )
            });
        painter.image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE.gamma_multiply(alpha),
        );
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
    id_salt: impl std::hash::Hash + std::fmt::Debug,
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
pub(super) fn site_of(url: &str) -> &str {
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
fn paint_site_badge(painter: &egui::Painter, rect: Rect, url: &str, alpha: f32, s: f32) {
    let site = site_name(site_of(url));
    let hash = site.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    let color = BADGE_COLORS[hash as usize % BADGE_COLORS.len()];
    painter.rect_filled(rect, 4.0 * s, color.gamma_multiply(alpha));
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
        Color32::from_rgb(240, 240, 244).gamma_multiply(alpha),
    );
}

#[cfg(test)]
mod tests {
    use super::{
        address_job, char_slice, ordered, replace_chars, single_line_text, site_name, site_of,
        TabDrag, TabDrop, TabList, TAB_GAP, TAB_HEIGHT,
    };
    use crate::canvas::CanvasState;
    use crate::preview::browser_chrome::{self as chrome_geometry, ChromeLayout};
    use crate::preview::{BrowserTab, BrowserTileStatus, FpsPreset, PreviewId, PreviewManager};

    fn list_for(tab_count: usize, tile_height: f32) -> TabList {
        let tile = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, tile_height));
        TabList::new(&ChromeLayout::new(tile, 1.0, false), tab_count)
    }

    #[test]
    fn tab_lists_scroll_only_when_they_overflow() {
        assert_eq!(list_for(3, 560.0).max_scroll, 0.0);
        let list = list_for(40, 560.0);
        let content = 41.0 * (TAB_HEIGHT + TAB_GAP) - TAB_GAP;
        assert!((list.max_scroll - (content - list.area.height())).abs() < 1e-3);
        // Scrolling moves the rows up by the same amount.
        assert!((list.row(5, 0.0).top() - list.row(5, 40.0).top() - 40.0).abs() < 1e-3);
    }

    #[test]
    fn dropped_tabs_land_in_the_nearest_gap() {
        let list = list_for(4, 560.0);
        let top = list.area.top();
        assert_eq!(list.slot_at(top + 2.0, 0.0, 4), 0);
        assert_eq!(list.slot_at(top + list.pitch * 1.6, 0.0, 4), 2);
        assert_eq!(list.slot_at(top + 10_000.0, 0.0, 4), 4);
        assert_eq!(list.slot_at(top - 50.0, 0.0, 4), 0);
        // With the list scrolled by a row, the same height is one gap later.
        assert_eq!(list.slot_at(top + 2.0, list.pitch, 4), 1);
    }
    use eframe::egui;

    /// Two framed browser tiles side by side, with `tab_counts` tabs.
    fn two_browsers(tab_counts: [usize; 2]) -> (PreviewManager, [PreviewId; 2]) {
        let mut previews = PreviewManager::new();
        let ids = [0.0, 700.0].map(|x| {
            previews.add_browser_placeholder(
                "https://example.com".to_owned(),
                egui::pos2(x, 0.0),
                egui::vec2(600.0, 400.0),
                FpsPreset::Medium,
            )
        });
        for (id, count) in ids.iter().zip(tab_counts) {
            let preview = previews.get_mut(*id).unwrap();
            preview.browser_status = BrowserTileStatus::Ready;
            preview.browser_chrome = true;
            preview.browser_tabs = (0..count)
                .map(|n| BrowserTab::new(format!("https://example.com/{n}")))
                .collect();
        }
        (previews, ids)
    }

    /// Release tab 0 of `from` at `pointer` and return what the canvas queued.
    fn drop_tab(previews: &PreviewManager, from: PreviewId, pointer: egui::Pos2) -> CanvasState {
        let mut canvas = CanvasState::default();
        let canvas_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0));
        canvas.last_screen_rect = Some(canvas_rect);
        canvas.browser_tab_drop = Some(TabDrop {
            drag: TabDrag { id: from, from: 0 },
            pointer,
        });
        canvas.follow_tab_drag(&egui::Context::default(), canvas_rect, previews);
        canvas
    }

    #[test]
    fn tabs_dropped_on_another_browser_join_its_tabs() {
        let (previews, [first, second]) = two_browsers([2, 3]);
        let target = previews.get(second).unwrap();
        let layout = chrome_geometry::layout_for(target, target.rect()).unwrap();
        let list = TabList::new(&layout, 3);

        // Over its tab strip, into the gap under the pointer.
        let canvas = drop_tab(&previews, first, list.row(1, 0.0).center());
        let transfer = canvas.pending_tab_transfers[0];
        assert_eq!((transfer.from, transfer.index), (first, 0));
        assert_eq!((transfer.to, transfer.slot), (second, 1));
        assert!(canvas.pending_tab_detaches.is_empty());

        // Anywhere else on it, after its last tab. A tile's only tab can go too.
        let (previews, [first, second]) = two_browsers([1, 3]);
        let canvas = drop_tab(&previews, first, layout.page.center());
        let transfer = canvas.pending_tab_transfers[0];
        assert_eq!((transfer.to, transfer.slot), (second, 3));
    }

    #[test]
    fn tabs_dropped_elsewhere_open_a_tile_but_a_last_tab_stays() {
        let empty_canvas = egui::pos2(1000.0, 700.0);
        let (previews, [first, _]) = two_browsers([2, 1]);
        let canvas = drop_tab(&previews, first, empty_canvas);
        assert!(canvas.pending_tab_transfers.is_empty());
        assert_eq!(canvas.pending_tab_detaches[0].index, 0);

        let (previews, [first, _]) = two_browsers([1, 1]);
        let canvas = drop_tab(&previews, first, empty_canvas);
        assert!(canvas.pending_tab_transfers.is_empty());
        assert!(canvas.pending_tab_detaches.is_empty());

        // A frozen browser tile cannot take tabs, so it is like empty canvas.
        let (mut previews, [first, second]) = two_browsers([2, 1]);
        previews.get_mut(second).unwrap().manually_frozen = true;
        let page = previews.get(second).unwrap().rect().center();
        let canvas = drop_tab(&previews, first, page);
        assert!(canvas.pending_tab_transfers.is_empty());
        assert_eq!(canvas.pending_tab_detaches.len(), 1);
    }

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
        assert_eq!(
            job.sections[0].byte_range,
            egui::text::ByteIndex(0)..egui::text::ByteIndex("twitch.tv".len())
        );
        let bare = address_job("https://example.com/", &font, 200.0);
        assert_eq!(bare.text, "example.com");
    }
}
