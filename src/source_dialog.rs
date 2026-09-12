//! Shared, compact forms for adding web sources. Source creation stays in app.rs.
use crate::ui_theme::{self, BORDER, GOLD, GOLD_DARK, HOVER, PANEL, SECONDARY, TEXT};
use eframe::egui::{self, RichText};

pub fn window(ctx: &egui::Context, title: &str) -> egui::Window<'static> {
    let width = (ctx.screen_rect().width() - 64.0).clamp(220.0, 440.0);
    egui::Window::new(title.to_owned())
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .min_width(width)
        .max_width(width)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(egui::Frame::window(&ctx.style()).inner_margin(20.0))
}

pub fn header(ui: &mut egui::Ui, icon: &str, title: &str, description: &str) -> bool {
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
    ui.spacing_mut().button_padding = egui::vec2(12.0, 8.0);
    let mut close = false;
    ui.horizontal_top(|ui| {
        let (badge, _) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::hover());
        ui.painter().rect_filled(badge, 9.0, GOLD_DARK);
        ui.painter().text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            egui::FontId::proportional(22.0),
            GOLD,
        );
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 36.0).max(100.0));
            ui.label(RichText::new(title).size(19.0).strong().color(TEXT));
            ui.label(RichText::new(description).size(12.5).color(SECONDARY));
        });
        close = ui
            .add_sized(
                [28.0, 28.0],
                egui::Button::new(egui_phosphor::regular::X).frame(false),
            )
            .on_hover_text("Close")
            .clicked();
    });
    ui.add_space(16.0);
    close
}

pub fn url_input(ui: &mut egui::Ui, url: &mut String, hint: &str) -> (egui::Response, bool) {
    ui_theme::section(ui, "URL");
    let mut paste = false;
    let response = ui
        .horizontal(|ui| {
            let response = ui.add_sized(
                [(ui.available_width() - 76.0).max(100.0), 36.0],
                egui::TextEdit::singleline(url)
                    .hint_text(hint)
                    .margin(egui::vec2(10.0, 9.0)),
            );
            paste = ui
                .add_sized([68.0, 36.0], egui::Button::new("Paste"))
                .clicked();
            response
        })
        .inner;
    response.context_menu(|ui| {
        ui_theme::menu(ui);
        if ui.button("Paste").clicked() {
            paste = true;
            ui.close_menu();
        }
    });
    (response, paste)
}

pub fn footer(ui: &mut egui::Ui, action: &str, enabled: bool) -> (bool, bool) {
    ui.add_space(14.0);
    ui.separator();
    ui.add_space(6.0);
    let mut submit = false;
    let mut cancel =
        ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 36.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            submit = ui
                .add_enabled(
                    enabled,
                    egui::Button::new(RichText::new(action).strong().color(PANEL))
                        .fill(GOLD)
                        .min_size(egui::vec2(112.0, 36.0)),
                )
                .clicked();
            cancel |= ui
                .add_sized([76.0, 36.0], egui::Button::new("Cancel").frame(false))
                .clicked();
        },
    );
    (submit, cancel)
}

pub fn recent_url(ui: &mut egui::Ui, url: &str) -> egui::Response {
    let address = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let split = address.find(['/', '?', '#']).unwrap_or(address.len());
    let domain = address[..split]
        .strip_prefix("www.")
        .unwrap_or(&address[..split]);
    let detail = if split == address.len() || &address[split..] == "/" {
        "Home page"
    } else {
        &address[split..]
    };
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("Open {url}"),
        )
    });
    if ui.is_rect_visible(rect) {
        let active = response.hovered() || response.has_focus();
        ui.painter().rect(
            rect,
            6.0,
            if active { HOVER } else { PANEL },
            egui::Stroke::new(1.0, if active { GOLD } else { BORDER }),
        );
        ui.painter().text(
            egui::pos2(rect.left() + 19.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            egui_phosphor::regular::GLOBE,
            egui::FontId::proportional(17.0),
            SECONDARY,
        );
        let left = rect.left() + 38.0;
        let right = rect.right() - 30.0;
        ui_theme::clipped_text(
            ui,
            egui::Rect::from_min_max(
                egui::pos2(left, rect.top() + 5.0),
                egui::pos2(right, rect.top() + 23.0),
            ),
            domain,
            13.5,
            TEXT,
        );
        ui_theme::clipped_text(
            ui,
            egui::Rect::from_min_max(
                egui::pos2(left, rect.top() + 25.0),
                egui::pos2(right, rect.bottom() - 3.0),
            ),
            detail,
            11.5,
            SECONDARY,
        );
        ui.painter().text(
            egui::pos2(rect.right() - 16.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            egui_phosphor::regular::ARROW_UP_RIGHT,
            egui::FontId::proportional(15.0),
            if active { GOLD } else { SECONDARY },
        );
    }
    response.on_hover_text(url)
}

pub fn quality_input(
    ui: &mut egui::Ui,
    quality: &mut String,
    detected: &[String],
) -> egui::Response {
    ui.add_space(12.0);
    ui_theme::section(ui, "QUALITY");
    let response = ui
        .horizontal(|ui| {
            ui.spacing_mut().interact_size.y = 36.0;
            let response = ui.add_sized(
                [(ui.available_width() - 132.0).max(100.0), 36.0],
                egui::TextEdit::singleline(quality)
                    .hint_text("best")
                    .margin(egui::vec2(10.0, 9.0)),
            );
            egui::ComboBox::from_id_salt("stream_quality_choices")
                .width(116.0)
                .selected_text("Choose")
                .show_ui(ui, |ui| {
                    ui_theme::menu(ui);
                    ui.selectable_value(quality, "best".to_owned(), "Best available");
                    ui.selectable_value(quality, "worst".to_owned(), "Lowest available");
                    let mut detected = detected
                        .iter()
                        .filter(|choice| !matches!(choice.as_str(), "best" | "worst"))
                        .peekable();
                    if detected.peek().is_some() {
                        ui.separator();
                        ui_theme::section(ui, "DETECTED");
                        for choice in detected {
                            ui.selectable_value(quality, choice.clone(), choice);
                        }
                    }
                });
            response
        })
        .inner;
    ui.label(
        RichText::new("Use best, choose a quality, or enter a custom value.")
            .size(12.0)
            .color(SECONDARY),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ui_theme::install(&ctx);
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        ctx.set_fonts(fonts);
        ctx
    }

    fn render(
        ctx: &egui::Context,
        width: f32,
        stream: bool,
        url: &mut String,
        quality: &mut String,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, egui::Rect, (bool, bool)) {
        let mut bounds = egui::Rect::NOTHING;
        let mut action = (false, false);
        let output = ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 640.0))),
            events, ..Default::default()
        }, |ctx| {
            let title = if stream { "Add Stream" } else { "Add Browser" };
            let result = window(ctx, title).show(ctx, |ui| {
                header(ui, if stream { egui_phosphor::regular::BROADCAST } else { egui_phosphor::regular::GLOBE },
                    title, if stream { "Play a live stream as a video tile." } else { "Websites, chats, and overlays on your canvas." });
                url_input(ui, url, "https://example.com/channel");
                if stream {
                    quality_input(ui, quality, &["best".into(), "1080p60".into(), "720p".into(), "worst".into()]);
                } else {
                    ui.add_space(14.0);
                    ui_theme::section(ui, "RECENT WEBSITES");
                    egui::ScrollArea::vertical().max_height(194.0).show(ui, |ui| {
                        for recent in ["https://www.twitch.tv/channel", "https://www.youtube.com/watch?v=example", "https://example.com/chat", "https://example.com/overlay", "https://example.com/a-very-long-path-that-must-never-widen-the-dialog?query=long"] {
                            recent_url(ui, recent);
                        }
                    });
                }
                action = footer(ui, if stream { "Add stream" } else { "Add browser" }, !url.trim().is_empty());
            }).unwrap();
            bounds = result.response.rect;
        });
        (output, bounds, action)
    }

    fn text_rect(output: &egui::FullOutput, label: &str) -> egui::Rect {
        output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == label => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Missing label: {label}"))
    }

    #[test]
    fn source_dialogs_stay_bounded_with_long_urls_on_small_viewports() {
        for width in [320.0, 800.0] {
            for stream in [false, true] {
                let ctx = context();
                let mut url = format!("https://example.com/{}", "long-path/".repeat(200));
                let mut quality = "best".to_owned();
                for frame in 0..8 {
                    let (output, bounds, _) =
                        render(&ctx, width, stream, &mut url, &mut quality, vec![]);
                    assert!(bounds.width() <= width - 8.0, "Dialog expanded: {bounds:?}");
                    assert!(
                        bounds.height() < 620.0,
                        "Dialog became too tall: {bounds:?}"
                    );
                    if stream && width >= 800.0 && frame > 1 {
                        assert!(
                            bounds.height() < 380.0,
                            "Stream form has excess empty space: {bounds:?}"
                        );
                    }
                    // The first sizing frame may not paint any text yet.
                    if output
                        .shapes
                        .iter()
                        .any(|s| matches!(s.shape, egui::Shape::Text(_)))
                    {
                        for label in [
                            "Paste",
                            "Cancel",
                            if stream { "Add stream" } else { "Add browser" },
                        ] {
                            assert!(
                                bounds.contains_rect(text_rect(&output, label)),
                                "Clipped {label}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn source_dialog_primary_action_rejects_blank_urls_and_escape_cancels() {
        for populated in [false, true] {
            let ctx = context();
            let mut url = if populated {
                "https://example.com"
            } else {
                "  "
            }
            .to_owned();
            let mut quality = "custom-quality".to_owned();
            let mut button = egui::Pos2::ZERO;
            for _ in 0..5 {
                let (output, _, _) = render(&ctx, 800.0, true, &mut url, &mut quality, vec![]);
                if output
                    .shapes
                    .iter()
                    .any(|s| matches!(s.shape, egui::Shape::Text(_)))
                {
                    button = text_rect(&output, "Add stream").center();
                }
            }
            let mut submitted = false;
            for pressed in [true, false] {
                let (_, _, action) = render(
                    &ctx,
                    800.0,
                    true,
                    &mut url,
                    &mut quality,
                    vec![
                        egui::Event::PointerMoved(button),
                        egui::Event::PointerButton {
                            pos: button,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
                submitted |= action.0;
            }
            assert_eq!(submitted, populated);
            assert_eq!(quality, "custom-quality");
            let (_, _, action) = render(
                &ctx,
                800.0,
                true,
                &mut url,
                &mut quality,
                vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert!(action.1);
        }
    }

    #[test]
    fn stream_quality_menu_selects_detected_and_preset_values() {
        let ctx = context();
        let mut url = "https://example.com/live".to_owned();
        let mut quality = "custom".to_owned();
        let mut output = None;
        for _ in 0..5 {
            output = Some(render(&ctx, 800.0, true, &mut url, &mut quality, vec![]).0);
        }
        for (label, expected) in [("1080p60", "1080p60"), ("Best available", "best")] {
            let choose = text_rect(output.as_ref().unwrap(), "Choose").center();
            for pressed in [true, false] {
                let _ = render(
                    &ctx,
                    800.0,
                    true,
                    &mut url,
                    &mut quality,
                    vec![
                        egui::Event::PointerMoved(choose),
                        egui::Event::PointerButton {
                            pos: choose,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            // Popups get a sizing pass before their first visible frame.
            output = Some(render(&ctx, 800.0, true, &mut url, &mut quality, vec![]).0);
            assert!(
                !output.as_ref().unwrap().shapes.iter().any(|shape| {
                    matches!(&shape.shape, egui::Shape::Text(text)
                    if matches!(text.galley.text(), "best" | "worst"))
                }),
                "Quality aliases should only appear as named presets"
            );
            let pos = text_rect(output.as_ref().unwrap(), label).center();
            for pressed in [true, false] {
                output = Some(
                    render(
                        &ctx,
                        800.0,
                        true,
                        &mut url,
                        &mut quality,
                        vec![
                            egui::Event::PointerMoved(pos),
                            egui::Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                    )
                    .0,
                );
            }
            assert_eq!(quality, expected);
        }
    }
}
