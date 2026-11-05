//! Shared chrome styling. Tile content and canvas geometry remain source-owned.
use crate::hotkeys::Hotkey;
use eframe::egui::{self, Color32, FontId, RichText, Stroke};

pub const PANEL: Color32 = Color32::from_rgb(22, 23, 27);
pub const SURFACE: Color32 = Color32::from_rgb(30, 31, 36);
pub const HOVER: Color32 = Color32::from_rgb(45, 44, 40);
pub const BORDER: Color32 = Color32::from_rgb(61, 61, 66);
pub const TEXT: Color32 = Color32::from_rgb(235, 233, 227);
pub const SECONDARY: Color32 = Color32::from_rgb(170, 172, 180);
pub const GOLD: Color32 = Color32::from_rgb(224, 181, 83);
pub const GOLD_DARK: Color32 = Color32::from_rgb(65, 53, 30);
pub const DANGER: Color32 = Color32::from_rgb(244, 151, 146);

pub fn install(ctx: &egui::Context) {
    ctx.global_style_mut(|style| {
        style.visuals = egui::Visuals::dark();
        let v = &mut style.visuals;
        v.panel_fill = PANEL;
        v.window_fill = SURFACE;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.menu_corner_radius = egui::CornerRadius::same(9);
        v.window_corner_radius = egui::CornerRadius::same(10);
        v.extreme_bg_color = PANEL;
        v.faint_bg_color = PANEL;
        v.selection.bg_fill = GOLD_DARK;
        v.selection.stroke = Stroke::new(1.0, GOLD);
        v.hyperlink_color = GOLD;
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.inactive.bg_fill = PANEL;
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(39, 40, 46);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        v.widgets.hovered.bg_fill = HOVER;
        v.widgets.hovered.weak_bg_fill = HOVER;
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, GOLD);
        v.widgets.active.bg_fill = GOLD_DARK;
        v.widgets.active.weak_bg_fill = GOLD_DARK;
        v.widgets.active.fg_stroke = Stroke::new(1.0, GOLD);
        for widget in [
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
        ] {
            widget.corner_radius = egui::CornerRadius::same(5);
            widget.expansion = 0.0;
        }
        style.spacing.menu_margin = egui::Margin::same(8);
        style.spacing.menu_width = 280.0;
    });
}

/// Apply inside each popup: egui resets button padding when opening a submenu.
pub fn menu(ui: &mut egui::Ui) {
    ui.set_min_width(184.0);
    // Only cap the remembered width; expanding it makes justified rows grow
    // again after the popup's initial measurement.
    ui.set_max_width(ui.available_width().min(280.0));
    let style = ui.style_mut();
    style.spacing.button_padding = egui::vec2(9.0, 4.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.item_spacing = egui::vec2(8.0, 3.0);
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(14.0));
}

/// Menus stay open until an item closes them with
/// `ui.close_kind(egui::UiKind::Menu)`, so toggles, sliders and steppers can be
/// adjusted in place. egui closes menus on any inside click by default since
/// 0.32; submenus inherit this from their root.
pub fn menu_config() -> egui::containers::menu::MenuConfig {
    egui::containers::menu::MenuConfig::new()
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
}

/// `Response::context_menu` with [`menu_config`]'s close behavior.
pub fn context_menu<R>(
    response: &egui::Response,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<egui::InnerResponse<R>> {
    egui::Popup::context_menu(response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(add_contents)
}

/// `Ui::menu_button` with [`menu_config`]'s close behavior when it is a menu
/// root. Inside a menu it opens a submenu, which inherits the root's config.
pub fn menu_button<'a, R>(
    ui: &mut egui::Ui,
    atoms: impl egui::IntoAtoms<'a>,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    if egui::containers::menu::is_in_menu(ui) {
        return ui.menu_button(atoms, add_contents);
    }
    let (response, inner) = egui::containers::menu::MenuButton::new(atoms)
        .config(menu_config())
        .ui(ui, add_contents);
    egui::InnerResponse::new(inner.map(|inner| inner.inner), response)
}

pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).size(11.5).strong().color(SECONDARY));
}

pub fn action(ui: &mut egui::Ui, icon: &str, title: &str) -> egui::Response {
    ui.button(format!("{icon}   {title}"))
}

pub fn destructive(ui: &mut egui::Ui, title: &str) -> egui::Response {
    ui.button(RichText::new(title).color(DANGER))
}

pub fn shortcut_button(
    title: impl Into<egui::WidgetText>,
    hotkey: Hotkey,
) -> egui::Button<'static> {
    egui::Button::new(title).shortcut_text(shortcut_text(hotkey))
}

pub fn shortcut_text(hotkey: Hotkey) -> RichText {
    RichText::new(hotkey.display())
        .monospace()
        .size(12.5)
        .color(Color32::from_rgb(196, 190, 173))
}

/// One measured row avoids nested layouts expanding a popup's sizing pass.
pub fn shortcut_toggle(
    ui: &mut egui::Ui,
    value: &mut bool,
    title: &str,
    hotkey: Hotkey,
) -> egui::Response {
    let hint = egui::WidgetText::from(shortcut_text(hotkey)).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    toggle_row(ui, value, title, Some(hint))
}

/// A check box menu row, like [`shortcut_toggle`] without a shortcut.
pub fn toggle(ui: &mut egui::Ui, value: &mut bool, title: &str) -> egui::Response {
    toggle_row(ui, value, title, None)
}

fn toggle_row(
    ui: &mut egui::Ui,
    value: &mut bool,
    title: &str,
    hint: Option<std::sync::Arc<egui::Galley>>,
) -> egui::Response {
    let title_width = ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(title.to_owned(), FontId::proportional(14.0), TEXT)
            .size()
            .x
    });
    let padding = ui.spacing().button_padding.x;
    let icon_width = ui.spacing().icon_width;
    let gap = ui.spacing().icon_spacing;
    let hint_width = hint.as_ref().map_or(0.0, |hint| hint.size().x);
    let desired_width = title_width + hint_width + icon_width + gap + 2.0 * padding + 20.0;
    let (rect, mut response) =
        ui.allocate_at_least(egui::vec2(desired_width, 28.0), egui::Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *value, title)
    });
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if response.hovered() || response.has_focus() {
            ui.painter().rect_filled(rect, 5.0, visuals.weak_bg_fill);
        }
        let icon = egui::Rect::from_center_size(
            egui::pos2(rect.left() + padding + icon_width * 0.5, rect.center().y),
            egui::vec2(icon_width, icon_width),
        );
        ui.painter().rect(
            icon,
            3.0,
            if *value { GOLD_DARK } else { PANEL },
            Stroke::new(1.0, if response.hovered() { GOLD } else { BORDER }),
            egui::StrokeKind::Middle,
        );
        if *value {
            let mark = icon.shrink(3.0);
            ui.painter().add(egui::Shape::line(
                vec![
                    mark.left_center(),
                    egui::pos2(mark.center().x, mark.bottom()),
                    mark.right_top(),
                ],
                Stroke::new(1.5, GOLD),
            ));
        }
        let title_right = match hint {
            Some(hint) => {
                let hint_pos = egui::pos2(
                    rect.right() - padding - hint.size().x,
                    rect.center().y - hint.size().y * 0.5,
                );
                ui.painter().galley(hint_pos, hint, SECONDARY);
                hint_pos.x - 20.0
            }
            None => rect.right() - padding,
        };
        let title_rect = egui::Rect::from_min_max(
            egui::pos2(icon.right() + gap, rect.center().y - 9.0),
            egui::pos2(title_right, rect.center().y + 9.0),
        );
        clipped_text(ui, title_rect, title, 14.0, visuals.text_color());
    }
    response
}

/// Use actual glyph widths for Unicode names, keeping text clear of trailing controls.
pub fn clipped_text(ui: &egui::Ui, rect: egui::Rect, text: &str, size: f32, color: Color32) {
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.to_owned(),
        FontId::proportional(size),
        color,
    );
    job.wrap.max_width = rect.width().max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.painter()
        .with_clip_rect(rect.intersect(ui.clip_rect()))
        .galley(rect.min, galley, color);
}

/// How long a toast that fades in over its first `fade_in` seconds and out
/// over its last `fade_out` can wait before its next frame. While it holds
/// still, nothing needs drawing until the fade-out begins.
pub fn toast_repaint_after(
    age: f32,
    lifetime: f32,
    fade_in: f32,
    fade_out: f32,
) -> std::time::Duration {
    let fade_out_starts = lifetime - fade_out;
    if age < fade_in || age >= fade_out_starts {
        std::time::Duration::ZERO
    } else {
        std::time::Duration::from_secs_f32(fade_out_starts - age)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_toast_redraws_only_while_it_fades() {
        let after = |age| toast_repaint_after(age, 4.0, 0.15, 0.5);
        assert_eq!(after(0.0), std::time::Duration::ZERO);
        assert_eq!(after(0.1), std::time::Duration::ZERO);
        // Holding still: the next frame is the start of the fade-out.
        let held = after(1.5).as_secs_f32();
        assert!((held - 2.0).abs() < 1e-4, "{held}");
        assert_eq!(after(3.5), std::time::Duration::ZERO);
        assert_eq!(after(3.9), std::time::Duration::ZERO);
    }

    #[test]
    fn context_menu_stays_open_for_toggles_and_closes_on_an_outside_click() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut checked = false;
        let mut render = |events| {
            let mut toggle = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let response = ui.allocate_rect(ui.max_rect(), egui::Sense::click());
                        context_menu(&response, |ui| {
                            menu(ui);
                            toggle = Some(
                                shortcut_toggle(ui, &mut checked, "Show Grid", Hotkey::key(0x47))
                                    .rect,
                            );
                        });
                    });
                },
            );
            toggle
        };
        let click = |render: &mut dyn FnMut(Vec<egui::Event>) -> Option<egui::Rect>,
                     pos: egui::Pos2,
                     button: egui::PointerButton| {
            render(vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                render(vec![egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }]);
            }
            render(vec![])
        };

        let toggle = click(
            &mut render,
            egui::pos2(300.0, 100.0),
            egui::PointerButton::Secondary,
        )
        .expect("menu opens");
        let after_toggle = click(&mut render, toggle.center(), egui::PointerButton::Primary);
        assert!(after_toggle.is_some(), "toggling must keep the menu open");
        let after_outside = click(
            &mut render,
            egui::pos2(700.0, 550.0),
            egui::PointerButton::Primary,
        );
        assert!(after_outside.is_none(), "a click outside closes the menu");
        assert!(checked, "the toggle click reached the toggle");
    }

    #[test]
    fn real_popup_with_shortcut_toggles_stays_compact_across_frames() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut checked = false;
        let pointer = egui::pos2(600.0, 100.0);
        let mut render = |events| {
            let mut bounds = None;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let response = ui.allocate_rect(ui.max_rect(), egui::Sense::click());
                        context_menu(&response, |ui| {
                            menu(ui);
                            shortcut_toggle(ui, &mut checked, "Window Picker", Hotkey::key(0x57));
                            shortcut_toggle(ui, &mut checked, "Show Grid", Hotkey::key(0x47));
                            let _ =
                                ui.add(shortcut_button("Keyboard Shortcuts", Hotkey::key(0x70)));
                            bounds = Some(ui.min_rect());
                        });
                    });
                },
            );
            (bounds, output)
        };
        render(vec![egui::Event::PointerMoved(pointer)]);
        for pressed in [true, false] {
            render(vec![egui::Event::PointerButton {
                pos: pointer,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        for _ in 0..8 {
            let (bounds, output) = render(vec![]);
            let bounds = bounds.expect("Popup should stay open");
            assert!(bounds.width() <= 240.0, "Popup became too wide: {bounds:?}");
            assert!(bounds.height() <= 100.0, "Popup rows expanded: {bounds:?}");
            let text_rect = |label: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == label => {
                            Some(text.galley.rect.translate(text.pos.to_vec2()))
                        }
                        _ => None,
                    })
                    .expect("Menu label must be painted")
            };
            assert!(
                (text_rect("Window Picker").left() - text_rect("Show Grid").left()).abs() <= 1.0
            );
            for key in ["W", "G", "F1"] {
                let hint = text_rect(key);
                assert!(bounds.contains_rect(hint));
                assert!((hint.right() - text_rect("F1").right()).abs() <= 1.0);
            }
        }
    }

    #[test]
    fn shortcut_hints_align_and_follow_custom_bindings() {
        use crate::hotkeys::{HotkeyBindings, HotkeySlot};
        let ctx = egui::Context::default();
        install(&ctx);
        let mut bindings = HotkeyBindings::default();
        bindings.set(HotkeySlot::ToggleGrid, Hotkey::pair(0x11, 0x22));
        let mut checked = false;
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 200.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.set_width(360.0);
                    menu(ui);
                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                        let _ = ui.add(shortcut_button(
                            "Keyboard Shortcuts",
                            bindings.get(HotkeySlot::ShowShortcutHelp),
                        ));
                        shortcut_toggle(
                            ui,
                            &mut checked,
                            "Show Grid",
                            bindings.get(HotkeySlot::ToggleGrid),
                        );
                    });
                });
            },
        );
        let text_rect = |label: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.galley.rect.translate(text.pos.to_vec2()))
                    }
                    _ => None,
                })
                .unwrap()
        };
        let help_hint = text_rect("F1");
        let grid_hint = text_rect("Ctrl+Page Down");
        assert!(
            (help_hint.right() - grid_hint.right()).abs() <= 1.0,
            "help: {help_hint:?}, grid: {grid_hint:?}"
        );
        assert!(text_rect("Show Grid").right() + 8.0 <= grid_hint.left());
        assert!(text_rect("Keyboard Shortcuts").right() + 8.0 <= help_hint.left());
    }

    #[test]
    fn shortcut_toggle_preserves_click_and_changed_semantics() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut checked = false;
        let mut render = |events| {
            let mut result = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        menu(ui);
                        result = Some(shortcut_toggle(
                            ui,
                            &mut checked,
                            "Show Grid",
                            Hotkey::key(0x47),
                        ));
                    });
                },
            );
            result.unwrap()
        };
        let pos = render(vec![]).rect.center();
        render(vec![egui::Event::PointerMoved(pos)]);
        render(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        let response = render(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert!(response.clicked());
        assert!(response.changed());
        assert!(checked);
    }

    #[test]
    fn chrome_text_has_readable_contrast_in_normal_and_selected_states() {
        fn luminance(color: Color32) -> f32 {
            let linear = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
        }
        for (text, background) in [
            (TEXT, SURFACE),
            (SECONDARY, SURFACE),
            (TEXT, HOVER),
            (GOLD, GOLD_DARK),
            (DANGER, SURFACE),
            (PANEL, GOLD),
        ] {
            let a = luminance(text);
            let b = luminance(background);
            assert!((a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5);
        }
    }

    #[test]
    fn long_unicode_source_text_is_elided_and_clipped_before_controls() {
        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(100.0, 20.0));
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                clipped_text(
                    ui,
                    rect,
                    "Écran — 日本語 — a very long source title",
                    14.0,
                    TEXT,
                );
            });
        });
        let shape = output
            .shapes
            .iter()
            .find(|shape| matches!(shape.shape, egui::Shape::Text(_)))
            .unwrap();
        let egui::Shape::Text(text) = &shape.shape else {
            unreachable!()
        };
        assert!(rect.contains_rect(shape.clip_rect));
        assert_eq!(text.galley.rows.len(), 1);
        assert!(text.galley.elided);
        assert!(text.galley.size().x <= rect.width());
    }
}
