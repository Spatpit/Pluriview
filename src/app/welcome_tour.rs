//! Getting Started tour: a few pages on the main features, each tip with a
//! short looping animation. It opens on the first launch (until closed once)
//! and from Help → Getting Started.

mod demos;

use super::PluriviewApp;
use crate::hotkeys::{HotkeyBindings, HotkeySlot};
use crate::ui_theme;
use demos::Demo;
use eframe::egui::{self, Align2, Color32, FontId, Key, RichText, Sense, Stroke, Vec2};
use egui_phosphor::regular as icon;

const WIDTH: f32 = 900.0;
const BODY_HEIGHT: f32 = 360.0;
/// Space the header, footer and margins take around the body.
const CHROME_HEIGHT: f32 = 280.0;
const ICON_SIZE: f32 = 28.0;
const HEADER_HEIGHT: f32 = 92.0;
const COLUMN_GAP: f32 = 20.0;
/// Room under the animation for its progress bar and hint.
const STAGE_FOOTER: f32 = 32.0;
/// Times a tip's animation plays before the next tip's starts.
const PLAYS: f32 = 2.0;
const KEY_TEXT: Color32 = Color32::from_rgb(196, 190, 173);
/// Inner margin and corner radius of the tour window.
const WINDOW_MARGIN: f32 = 24.0;
const ROUNDING: f32 = 12.0;
/// Width of the page badge and the space after it in the header.
const HEADER_BADGE: f32 = 60.0;
/// Mira, Pluriview's mascot, peeks in from the header's right side: a strip
/// of her seated artwork from her hair clip to her chin, its height filling
/// the header, fading into the panel behind the title.
const BANNER: &[u8] = include_bytes!("../../assets/mira/tour-banner.webp");
/// Mira's Spout idle layers, for the demos' VTuber feed.
const SPOUT: &[u8] = include_bytes!("../../assets/mira/tour-spout.webp");
/// Share of the header the title and intro may use; Mira has the rest.
const HEADER_TEXT: f32 = 0.52;
/// How far the art reaches left under the text, and how far past the text
/// it keeps fading in, in points.
const BANNER_UNDER_TEXT: f32 = 120.0;
const BANNER_FADE: f32 = 260.0;

/// A shortcut shown beside a tip: the user's current binding, or a fixed
/// mouse gesture.
#[derive(Clone, Copy)]
enum Keys {
    Slot(HotkeySlot),
    Fixed(&'static str),
}

impl Keys {
    fn label(self, bindings: &HotkeyBindings) -> String {
        match self {
            Self::Slot(slot) => bindings.get(slot).display(),
            Self::Fixed(text) => text.to_owned(),
        }
    }
}

struct Tip {
    icon: &'static str,
    title: &'static str,
    text: &'static str,
    keys: Option<Keys>,
    demo: Demo,
}

struct Page {
    icon: &'static str,
    title: &'static str,
    intro: &'static str,
    tips: &'static [Tip],
}

const fn tip(icon: &'static str, title: &'static str, text: &'static str, demo: Demo) -> Tip {
    Tip {
        icon,
        title,
        text,
        keys: None,
        demo,
    }
}

const fn tip_key(
    icon: &'static str,
    title: &'static str,
    text: &'static str,
    keys: Keys,
    demo: Demo,
) -> Tip {
    Tip {
        icon,
        title,
        text,
        keys: Some(keys),
        demo,
    }
}

const PAGES: [Page; 8] = [
    Page {
        icon: icon::HAND_WAVING,
        title: "Welcome to Pluriview",
        intro: "Put your app windows, web pages, videos and images side by side on one \
                infinite canvas.",
        tips: &[
            tip(
                icon::SQUARES_FOUR,
                "Everything is a tile",
                "Each thing you add becomes a tile you can move, resize, crop and group.",
                Demo::Tiles,
            ),
            tip(
                icon::LIGHTNING,
                "Tiles stay live",
                "Window tiles show the app as it runs, and you can use the app right from \
                 its tile.",
                Demo::Live,
            ),
            tip(
                icon::FOLDERS,
                "Saved as you go",
                "Your layout saves automatically. Workspaces keep separate canvases for \
                 different activities.",
                Demo::Saved,
            ),
        ],
    },
    Page {
        icon: icon::PLUS_CIRCLE,
        title: "Add something to the canvas",
        intro: "Start with a window, a web page or a file. You can mix them freely.",
        tips: &[
            tip_key(
                icon::APP_WINDOW,
                "App windows",
                "Pick any open window from the Window Picker on the left, or right-click \
                 empty canvas → Add Window...",
                Keys::Slot(HotkeySlot::ToggleWindowPicker),
                Demo::AddWindow,
            ),
            tip(
                icon::GLOBE,
                "Web pages",
                "Right-click empty canvas → Add Browser... Pages get tabs, sign-ins and \
                 sound, and transparent widgets stay transparent.",
                Demo::AddBrowser,
            ),
            tip(
                icon::FILM_STRIP,
                "Images, GIFs and videos",
                "Drag files onto the canvas. Drop a folder of videos to make a playlist.",
                Demo::DropFiles,
            ),
            tip(
                icon::BROADCAST,
                "Streams and Spout2",
                "Open a stream site in a web page tile. Spout2 senders such as VTube \
                 Studio appear in the Window Picker.",
                Demo::Streams,
            ),
        ],
    },
    Page {
        icon: icon::ARROWS_OUT_CARDINAL,
        title: "Move around and arrange",
        intro: "The canvas has no edges. Pan and zoom to make room, then place tiles \
                wherever you like.",
        tips: &[
            tip_key(
                icon::MOUSE_MIDDLE_CLICK,
                "Pan and zoom",
                "Drag with the middle mouse button to pan, and scroll to zoom.",
                Keys::Fixed("Middle drag"),
                Demo::PanZoom,
            ),
            tip(
                icon::CORNERS_OUT,
                "Move and resize",
                "Drag a tile to move it. Drag its edges or corners to resize it.",
                Demo::MoveResize,
            ),
            tip_key(
                icon::SELECTION,
                "Select several",
                "Ctrl+click tiles, or drag a box on empty canvas, to work on them together.",
                Keys::Slot(HotkeySlot::SelectAll),
                Demo::Select,
            ),
            tip_key(
                icon::CROP,
                "Crop",
                "Hold Alt and drag a tile's edge to show only the part you need.",
                Keys::Fixed("Alt+drag"),
                Demo::Crop,
            ),
            tip_key(
                icon::TRASH,
                "Remove and undo",
                "Delete removes the selected tiles. Ctrl+Z brings them back.",
                Keys::Slot(HotkeySlot::DeleteSelected),
                Demo::Remove,
            ),
        ],
    },
    Page {
        icon: icon::CURSOR_CLICK,
        title: "Use your apps and pages",
        intro: "Tiles aren't just pictures: you can click, type and scroll in them \
                without leaving the canvas.",
        tips: &[
            tip_key(
                icon::APP_WINDOW,
                "Use an app",
                "Double-click a window tile and the real app moves over it, at its real \
                 size. Click outside it when you're done.",
                Keys::Fixed("Double-click"),
                Demo::UseApp,
            ),
            tip_key(
                icon::BROWSER,
                "Use a web page",
                "Double-click a browser tile to click and type in the page. Esc goes back \
                 to arranging.",
                Keys::Slot(HotkeySlot::InteractBrowser),
                Demo::UsePage,
            ),
            tip(
                icon::CURSOR,
                "Hover for controls",
                "Hover a tile for its quick controls, such as play, pause and volume on \
                 videos.",
                Demo::Hover,
            ),
            tip_key(
                icon::MOUSE_RIGHT_CLICK,
                "Right-click for everything else",
                "Crop, frame rate, freeze, pin and more live in each tile's menu.",
                Keys::Fixed("Right-click"),
                Demo::TileMenu,
            ),
        ],
    },
    Page {
        icon: icon::STACK,
        title: "Stay organized",
        intro: "As the canvas fills up, these keep things tidy and easy to find.",
        tips: &[
            tip_key(
                icon::MAGNIFYING_GLASS_PLUS,
                "Focus",
                "Fill the view with the selected tile. Esc goes back.",
                Keys::Slot(HotkeySlot::FocusCurrentTile),
                Demo::Focus,
            ),
            tip_key(
                icon::SQUARES_FOUR,
                "Groups",
                "Group selected tiles so they move together. Alt+1 to Alt+4 arrange them \
                 with large tiles or all equal.",
                Keys::Slot(HotkeySlot::GroupTiles),
                Demo::Group,
            ),
            tip_key(
                icon::MAP_TRIFOLD,
                "Control Tile Center",
                "A separate window listing every tile, with a map of the whole canvas.",
                Keys::Slot(HotkeySlot::ToggleTileControl),
                Demo::TileCenter,
            ),
            tip(
                icon::PUSH_PIN,
                "Pin to Viewport",
                "Right-click a tile → Pin to Viewport to keep it on screen while you pan \
                 and zoom.",
                Demo::Pin,
            ),
            tip_key(
                icon::PICTURE_IN_PICTURE,
                "Picture in Picture",
                "Pop tiles out into a small window that stays above other apps and games.",
                Keys::Slot(HotkeySlot::PictureInPicture),
                Demo::Pip,
            ),
        ],
    },
    Page {
        icon: icon::CAMERA,
        title: "Capture and share",
        intro: "Save what's on your canvas, or share it with friends and viewers.",
        tips: &[
            tip_key(
                icon::CAMERA,
                "Screenshots",
                "Save the selected tiles as a PNG, laid out as they sit on the canvas.",
                Keys::Slot(HotkeySlot::ScreenshotTile),
                Demo::Screenshot,
            ),
            tip_key(
                icon::RECORD,
                "Recording",
                "Record the selected tiles to an MP4 with sound. Press it again to stop.",
                Keys::Slot(HotkeySlot::RecordTile),
                Demo::Record,
            ),
            tip_key(
                icon::IMAGES,
                "Screenshots panel",
                "Browse and sort your shots, or drag them back onto the canvas.",
                Keys::Slot(HotkeySlot::ToggleScreenshotsPanel),
                Demo::ScreenshotPanel,
            ),
            tip(
                icon::SPEAKER_HIGH,
                "Sound in Discord or OBS",
                "Tools → Stream Audio Monitor adds web page sound when you share the \
                 Pluriview window.",
                Demo::StreamAudio,
            ),
            tip(
                icon::DEVICE_MOBILE,
                "Phone viewer",
                "Tools → Phone Viewer... shows your canvas on an Android phone over Wi-Fi.",
                Demo::Phone,
            ),
        ],
    },
    Page {
        icon: icon::ROCKET_LAUNCH,
        title: "Make it yours",
        intro: "A few more ways to set Pluriview up the way you like it.",
        tips: &[
            tip(
                icon::BOOKMARK_SIMPLE,
                "Saved viewpoints",
                "View → Saved Viewpoints bookmarks spots on the canvas. Ctrl+1 to Ctrl+9 \
                 jump to them.",
                Demo::Viewpoints,
            ),
            tip(
                icon::IMAGE,
                "Wallpaper",
                "View → Set Wallpaper... puts an image, GIF or looping video behind the \
                 canvas.",
                Demo::Wallpaper,
            ),
            tip(
                icon::FRAME_CORNERS,
                "Overlay mode",
                "Right-click empty canvas → Always on Top keeps Pluriview over a game or \
                 app. Tick Click Through in the tray icon's menu and your clicks pass \
                 through Pluriview to the app behind. Untick it there to use Pluriview \
                 again.",
                Demo::Overlay,
            ),
            tip_key(
                icon::EYE_SLASH,
                "Hide the interface",
                "Show only the canvas, for a clean view or a stream.",
                Keys::Slot(HotkeySlot::ToggleCanvasOnly),
                Demo::HideUi,
            ),
        ],
    },
    Page {
        icon: icon::CONFETTI,
        title: "You're all set",
        intro: "That's the essentials. Right-click empty canvas whenever you want to add \
                something.",
        tips: &[
            tip(
                icon::LIST_BULLETS,
                "Come back any time",
                "Reopen this tour from Help → Getting Started.",
                Demo::Reopen,
            ),
            tip_key(
                icon::KEYBOARD,
                "All shortcuts",
                "See every shortcut here, and change them in Tools → Settings...",
                Keys::Slot(HotkeySlot::ShowShortcutHelp),
                Demo::Shortcuts,
            ),
        ],
    },
];

#[derive(Default)]
pub(super) struct WelcomeTour {
    page: usize,
    /// The tip whose animation is playing; it alone shows its description.
    tip: usize,
    /// When the current tip's animation started, in egui seconds.
    started: Option<f64>,
    /// The user picked a tip, so tips no longer advance on their own.
    chosen: bool,
    /// The pointer was over the tip list last frame, which holds the tip.
    holding: bool,
    /// Mira's banner art, decoded when the tour first draws (`Some(None)`
    /// if that failed, so it isn't retried every frame).
    banner: Option<Option<egui::TextureHandle>>,
    /// Mira's Spout sprite for the demos, loaded the same way.
    spout: Option<Option<egui::TextureHandle>>,
    /// The current page needed scrolling in the last frame.
    #[cfg(test)]
    overflowed: bool,
}

impl WelcomeTour {
    fn go_to(&mut self, page: usize) {
        self.page = page;
        self.tip = 0;
        self.started = None;
        self.chosen = false;
    }

    fn choose(&mut self, tip: usize) {
        self.tip = tip;
        self.started = None;
        self.chosen = true;
    }

    /// Draw the tour. Returns true when the user closes it.
    fn ui(
        &mut self,
        ctx: &egui::Context,
        bindings: &HotkeyBindings,
        brand: Option<egui::TextureId>,
    ) -> bool {
        let last = PAGES.len() - 1;
        let mut close = false;
        if !ctx.egui_wants_keyboard_input() {
            let tips = PAGES[self.page].tips.len();
            ctx.input(|input| {
                if input.key_pressed(Key::Escape) {
                    close = true;
                } else if input.key_pressed(Key::ArrowRight) || input.key_pressed(Key::Enter) {
                    if self.page == last {
                        close = true;
                    } else {
                        self.go_to(self.page + 1);
                    }
                } else if input.key_pressed(Key::ArrowLeft) {
                    self.go_to(self.page.saturating_sub(1));
                } else if input.key_pressed(Key::ArrowDown) {
                    self.choose((self.tip + 1).min(tips - 1));
                } else if input.key_pressed(Key::ArrowUp) {
                    self.choose(self.tip.saturating_sub(1));
                }
            });
        }

        let now = ctx.input(|input| input.time);
        let mut elapsed = (now - *self.started.get_or_insert(now)) as f32;
        let length = PAGES[self.page].tips[self.tip].demo.length();
        if !self.chosen && !self.holding && elapsed >= length * PLAYS {
            self.tip = (self.tip + 1) % PAGES[self.page].tips.len();
            self.started = Some(now);
            elapsed = 0.0;
        }
        // The demos animate on every frame, but a minimized window shows
        // nothing; restoring it repaints and the demos carry on.
        if !ctx.input(|input| input.viewport().minimized.unwrap_or(false)) {
            ctx.request_repaint();
        }

        let screen = ctx.content_rect();
        let width = WIDTH.min(screen.width() - 96.0).max(320.0);
        let body_height = BODY_HEIGHT.min(screen.height() - CHROME_HEIGHT).max(140.0);
        let stage_width = (width * 0.5)
            .min((body_height - STAGE_FOOTER) / 0.75)
            .max(120.0);
        let list_width = width - stage_width - COLUMN_GAP;
        let page = &PAGES[self.page];
        let banner = self
            .banner
            .get_or_insert_with(|| load_art(ctx, "welcome_tour_banner", BANNER))
            .as_ref()
            .map(|texture| (texture.id(), texture.size_vec2()));

        egui::Window::new("Getting Started")
            .id(egui::Id::new("welcome_tour"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(&ctx.global_style())
                    .fill(ui_theme::PANEL)
                    .stroke(Stroke::new(1.0, ui_theme::BORDER))
                    .corner_radius(ROUNDING)
                    .inner_margin(WINDOW_MARGIN),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
                // Reserved first so the art is painted under the header.
                let art = ui.painter().add(egui::Shape::Noop);
                self.header(ui, page, brand, &mut close);
                ui.add_space(14.0);
                let body_top = ui.cursor().top();
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = COLUMN_GAP;
                    let list = ui.allocate_ui_with_layout(
                        Vec2::new(list_width, body_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(list_width);
                            egui::ScrollArea::vertical()
                                .id_salt(("welcome_tour_page", self.page))
                                .max_height(body_height)
                                .min_scrolled_height(body_height)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = 4.0;
                                    let mut chosen = None;
                                    for (index, tip) in page.tips.iter().enumerate() {
                                        if tip_row(ui, tip, bindings, index == self.tip).clicked() {
                                            chosen = Some(index);
                                        }
                                    }
                                    chosen
                                })
                        },
                    );
                    self.holding = list.response.contains_pointer();
                    if let Some(index) = list.inner.inner {
                        self.choose(index);
                    }
                    #[cfg(test)]
                    {
                        let body = &list.inner;
                        self.overflowed = body.content_size.y > body.inner_rect.height() + 0.5;
                    }
                    let (area, _) =
                        ui.allocate_exact_size(Vec2::new(stage_width, body_height), Sense::hover());
                    let spout = self
                        .spout
                        .get_or_insert_with(|| load_art(ctx, "welcome_tour_spout", SPOUT))
                        .as_ref()
                        .map(egui::TextureHandle::id);
                    self.stage(ui, area, elapsed, bindings, spout);
                });
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                self.footer(ui, &mut close);
                if let Some(banner) = banner {
                    let window = ui.min_rect().expand(WINDOW_MARGIN);
                    paint_banner(ui, art, banner, window, body_top);
                }
            });
        close
    }

    /// The current tip's animation, with a progress bar and a hint below.
    fn stage(
        &self,
        ui: &egui::Ui,
        area: egui::Rect,
        elapsed: f32,
        bindings: &HotkeyBindings,
        spout: Option<egui::TextureId>,
    ) {
        let stage =
            egui::Rect::from_min_size(area.min, Vec2::new(area.width(), area.width() * 0.75));
        let tip = &PAGES[self.page].tips[self.tip];
        demos::paint(ui.painter(), stage, tip.demo, elapsed, bindings, spout);
        let length = tip.demo.length();

        let track = egui::Rect::from_min_size(
            egui::pos2(stage.min.x, stage.max.y + 8.0),
            Vec2::new(stage.width(), 3.0),
        );
        let progress = if self.chosen {
            elapsed.rem_euclid(length) / length
        } else {
            (elapsed / (length * PLAYS)).min(1.0)
        };
        let painter = ui.painter();
        painter.rect_filled(track, 1.5, ui_theme::SURFACE);
        painter.rect_filled(
            egui::Rect::from_min_size(track.min, Vec2::new(track.width() * progress, 3.0)),
            1.5,
            ui_theme::GOLD.gamma_multiply(0.7),
        );
        if track.max.y + 16.0 <= area.max.y {
            painter.text(
                egui::pos2(stage.center().x, track.max.y + 9.0),
                Align2::CENTER_CENTER,
                "Click a tip to see it in action",
                FontId::proportional(11.5),
                ui_theme::SECONDARY,
            );
        }
    }

    fn header(
        &self,
        ui: &mut egui::Ui,
        page: &Page,
        brand: Option<egui::TextureId>,
        close: &mut bool,
    ) {
        let text_width = header_text_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let (badge, _) = ui.allocate_exact_size(Vec2::splat(52.0), Sense::hover());
            match brand.filter(|_| self.page == 0) {
                Some(texture) => {
                    egui::Image::new((texture, badge.size())).paint_at(ui, badge);
                }
                None => {
                    ui.painter().rect_filled(badge, 12.0, ui_theme::GOLD_DARK);
                    ui.painter().text(
                        badge.center(),
                        Align2::CENTER_CENTER,
                        page.icon,
                        FontId::proportional(26.0),
                        ui_theme::GOLD,
                    );
                }
            }
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.set_width(text_width);
                // Room for a two-line intro, so pages don't change height.
                ui.set_min_height(HEADER_HEIGHT);
                ui.label(
                    RichText::new(format!("STEP {} OF {}", self.page + 1, PAGES.len()))
                        .size(11.5)
                        .strong()
                        .color(ui_theme::GOLD),
                );
                ui.label(
                    RichText::new(page.title)
                        .size(22.0)
                        .strong()
                        .color(ui_theme::TEXT),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(arrows(page.intro))
                            .size(14.0)
                            .color(ui_theme::SECONDARY),
                    )
                    .wrap(),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                // A dark disc keeps the button readable over Mira's art.
                let backing = ui.painter().add(egui::Shape::Noop);
                let close_button = ui
                    .add(
                        egui::Button::new(RichText::new(icon::X).size(16.0).color(ui_theme::TEXT))
                            .frame(false),
                    )
                    .on_hover_text("Close the tour");
                let disc = if close_button.hovered() { 200 } else { 130 };
                ui.painter().set(
                    backing,
                    egui::Shape::circle_filled(
                        close_button.rect.center(),
                        15.0,
                        Color32::from_black_alpha(disc),
                    ),
                );
                if close_button.clicked() {
                    *close = true;
                }
            });
        });
    }

    fn footer(&mut self, ui: &mut egui::Ui, close: &mut bool) {
        const BUTTON_HEIGHT: f32 = 32.0;
        const NEXT_WIDTH: f32 = 170.0;
        const BACK_WIDTH: f32 = 90.0;
        let last = PAGES.len() - 1;
        let (row, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), BUTTON_HEIGHT),
            Sense::hover(),
        );

        if self.page < last {
            let skip = egui::Rect::from_min_size(row.min, Vec2::new(90.0, BUTTON_HEIGHT));
            let skip = ui.put(
                skip,
                egui::Button::new(RichText::new("Skip tour").color(ui_theme::SECONDARY))
                    .frame(false),
            );
            if skip.clicked() {
                *close = true;
            }
        }

        let spacing = 16.0;
        let first_x = row.center().x - spacing * last as f32 / 2.0;
        for (index, page) in PAGES.iter().enumerate() {
            let center = egui::pos2(first_x + spacing * index as f32, row.center().y);
            let dot = ui
                .interact(
                    egui::Rect::from_center_size(center, Vec2::splat(14.0)),
                    ui.id().with(("welcome_tour_dot", index)),
                    Sense::click(),
                )
                .on_hover_text(page.title);
            if dot.clicked() {
                self.go_to(index);
            }
            let (radius, color) = if index == self.page {
                (4.5, ui_theme::GOLD)
            } else if dot.hovered() {
                (3.5, ui_theme::SECONDARY)
            } else {
                (3.5, ui_theme::BORDER)
            };
            ui.painter().circle_filled(center, radius, color);
        }

        let next = egui::Rect::from_min_size(
            egui::pos2(row.right() - NEXT_WIDTH, row.top()),
            Vec2::new(NEXT_WIDTH, BUTTON_HEIGHT),
        );
        let next_label = if self.page == last {
            "Start using Pluriview"
        } else {
            "Next"
        };
        let next = ui.put(
            next,
            egui::Button::new(
                RichText::new(next_label)
                    .size(14.0)
                    .strong()
                    .color(ui_theme::PANEL),
            )
            .fill(ui_theme::GOLD)
            .corner_radius(6.0),
        );
        if next.clicked() {
            if self.page == last {
                *close = true;
            } else {
                self.go_to(self.page + 1);
            }
        }

        if self.page > 0 {
            let back = egui::Rect::from_min_size(
                egui::pos2(row.right() - NEXT_WIDTH - 8.0 - BACK_WIDTH, row.top()),
                Vec2::new(BACK_WIDTH, BUTTON_HEIGHT),
            );
            if ui
                .put(back, egui::Button::new("Back").corner_radius(6.0))
                .clicked()
            {
                self.go_to(self.page - 1);
            }
        }
    }
}

/// Decode one of Mira's embedded WebP images into a texture.
fn load_art(ctx: &egui::Context, name: &str, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP)
        .ok()?
        .into_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(ctx.load_texture(
        name,
        egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
        // Both are drawn smaller than their pixels.
        egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear)),
    ))
}

/// Width of the header's title and intro, beside the page badge, so they
/// stay clear of Mira.
fn header_text_width(content_width: f32) -> f32 {
    (content_width - HEADER_BADGE) * HEADER_TEXT
}

/// Mira's art across the header's right side, from the window's top edge
/// down to the body, painted into `slot`, reserved under the header.
fn paint_banner(
    ui: &egui::Ui,
    slot: egui::layers::ShapeIdx,
    (texture, size): (egui::TextureId, Vec2),
    window: egui::Rect,
    bottom: f32,
) {
    let content = window.shrink(WINDOW_MARGIN);
    let text_right = content.left() + HEADER_BADGE + header_text_width(content.width());
    // Inside the window's 1-point border.
    let art = egui::Rect::from_min_max(
        egui::pos2(
            (text_right - BANNER_UNDER_TEXT).max(window.left() + 1.0),
            window.top() + 1.0,
        ),
        egui::pos2(window.right() - 1.0, bottom),
    );
    // The strip's right end meets the window's right edge.
    let columns = art.width() * size.y / art.height();
    let uv = egui::Rect::from_min_max(
        egui::pos2((size.x - columns) / size.x, 0.0),
        egui::pos2(1.0, 1.0),
    );
    let mut shape = egui::epaint::RectShape::filled(
        art,
        egui::epaint::CornerRadiusF32 {
            ne: ROUNDING - 1.0,
            ..Default::default()
        },
        Color32::WHITE,
    );
    shape = shape.with_texture(texture, uv);
    // Starts just outside the art so its anti-aliased edge doesn't show.
    let left = egui::Rect::from_min_size(
        art.min - Vec2::new(2.0, 0.0),
        Vec2::new(BANNER_UNDER_TEXT + BANNER_FADE, art.height()),
    );
    let bottom = egui::Rect::from_min_max(egui::pos2(art.left(), art.bottom() - 34.0), art.max);
    let mut painter = ui.painter().clone();
    painter.set_clip_rect(window);
    painter.set(
        slot,
        egui::Shape::Vec(vec![shape.into(), fade(left, false), fade(bottom, true)]),
    );
}

/// Panel colour over `rect`, opaque at its left edge (or bottom edge, if
/// `up`) and clear at the opposite one, eased so neither edge shows.
fn fade(rect: egui::Rect, up: bool) -> egui::Shape {
    const STEPS: u32 = 16;
    let mut mesh = egui::Mesh::default();
    for i in 0..=STEPS {
        let t = i as f32 / STEPS as f32;
        let color = ui_theme::PANEL.gamma_multiply(1.0 - t * t * (3.0 - 2.0 * t));
        let (a, b) = if up {
            let y = egui::lerp(rect.bottom()..=rect.top(), t);
            (egui::pos2(rect.left(), y), egui::pos2(rect.right(), y))
        } else {
            let x = egui::lerp(rect.left()..=rect.right(), t);
            (egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom()))
        };
        mesh.colored_vertex(a, color);
        mesh.colored_vertex(b, color);
        if i > 0 {
            let n = 2 * i;
            mesh.add_triangle(n - 2, n - 1, n);
            mesh.add_triangle(n - 1, n, n + 1);
        }
    }
    egui::Shape::mesh(mesh)
}

/// `text` with each menu-path arrow drawn from the icon font: the default
/// text font has no arrow glyph.
fn arrows(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains('→') {
        text.replace('→', icon::ARROW_RIGHT).into()
    } else {
        text.into()
    }
}

/// One tip in the list. Only the active tip shows its description.
fn tip_row(
    ui: &mut egui::Ui,
    tip: &Tip,
    bindings: &HotkeyBindings,
    active: bool,
) -> egui::Response {
    let keys = tip.keys.map(|keys| {
        ui.fonts_mut(|fonts| {
            fonts.layout_no_wrap(keys.label(bindings), FontId::monospace(12.0), KEY_TEXT)
        })
    });
    let frame = egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 6))
        .corner_radius(8.0)
        .fill(if active {
            ui_theme::SURFACE
        } else {
            Color32::TRANSPARENT
        });
    let row = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let (badge, _) = ui.allocate_exact_size(Vec2::splat(ICON_SIZE), Sense::hover());
            ui.painter().rect_filled(
                badge,
                7.0,
                if active {
                    ui_theme::GOLD_DARK
                } else {
                    ui_theme::SURFACE
                },
            );
            ui.painter().text(
                badge.center(),
                Align2::CENTER_CENTER,
                tip.icon,
                FontId::proportional(15.0),
                ui_theme::GOLD,
            );
            ui.add_space(2.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.horizontal(|ui| {
                    ui.set_min_height(ICON_SIZE);
                    let color = if active {
                        ui_theme::TEXT
                    } else {
                        ui_theme::SECONDARY
                    };
                    ui.label(RichText::new(tip.title).size(14.5).strong().color(color));
                    if let Some(keys) = keys {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let (chip, _) = ui.allocate_exact_size(
                                keys.size() + Vec2::new(14.0, 8.0),
                                Sense::hover(),
                            );
                            ui.painter().rect(
                                chip,
                                5.0,
                                ui_theme::PANEL,
                                Stroke::new(1.0, ui_theme::BORDER),
                                egui::StrokeKind::Middle,
                            );
                            ui.painter()
                                .galley(chip.center() - keys.size() / 2.0, keys, KEY_TEXT);
                        });
                    }
                });
                if active {
                    ui.add(
                        egui::Label::new(
                            RichText::new(arrows(tip.text))
                                .size(13.0)
                                .color(ui_theme::SECONDARY),
                        )
                        .wrap(),
                    );
                }
            });
        });
    });
    let response = row
        .response
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let rect = response.rect;
    if active {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                rect.min + Vec2::new(0.0, 8.0),
                Vec2::new(3.0, rect.height() - 16.0),
            ),
            1.5,
            ui_theme::GOLD,
        );
    } else if response.hovered() {
        ui.painter().rect_stroke(
            rect,
            8.0,
            Stroke::new(1.0, ui_theme::BORDER),
            egui::StrokeKind::Middle,
        );
    }
    response
}

impl PluriviewApp {
    pub(super) fn open_welcome_tour(&mut self) {
        self.welcome_tour = Some(WelcomeTour::default());
    }

    pub(super) fn welcome_tour_ui(&mut self, ctx: &egui::Context) {
        let Some(tour) = &mut self.welcome_tour else {
            return;
        };
        let brand = Some(self.brand_icon.id());
        if tour.ui(ctx, &self.app_config.keyboard_shortcuts, brand) {
            self.welcome_tour = None;
            if !self.app_config.welcome_tour_seen {
                self.app_config.welcome_tour_seen = true;
                self.save_app_config();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::demos::{self, Demo, LOOP};
    use super::{load_art, WelcomeTour, BANNER, PAGES, PLAYS, SPOUT};
    use crate::hotkeys::HotkeyBindings;
    use crate::ui_theme;
    use eframe::egui::{self, Key, Pos2, Rect, Vec2};

    fn key(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn frame_at(
        ctx: &egui::Context,
        tour: &mut WelcomeTour,
        size: Vec2,
        time: f64,
        events: Vec<egui::Event>,
    ) -> (bool, Rect) {
        let mut close = false;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                let ctx = &ui.ctx().clone();
                close = tour.ui(ctx, &HotkeyBindings::default(), None)
            },
        );
        let window = ctx
            .memory(|memory| memory.area_rect(egui::Id::new("welcome_tour")))
            .unwrap();
        (close, window)
    }

    fn frame(
        ctx: &egui::Context,
        tour: &mut WelcomeTour,
        size: Vec2,
        events: Vec<egui::Event>,
    ) -> (bool, Rect) {
        frame_at(ctx, tour, size, 0.0, events)
    }

    #[test]
    fn every_tip_fits_without_scrolling_and_inside_the_smallest_window() {
        let ctx = egui::Context::default();
        ui_theme::install(&ctx);
        // egui's clock never runs backwards.
        let mut clock = 0.0;
        for size in [Vec2::new(1280.0, 800.0), Vec2::new(800.0, 600.0)] {
            let mut tour = WelcomeTour::default();
            let mut window_size = None;
            for (page, entry) in PAGES.iter().enumerate() {
                for tip in 0..entry.tips.len() {
                    tour.go_to(page);
                    tour.choose(tip);
                    // Windows settle their size over a couple of frames.
                    for _ in 0..3 {
                        frame_at(&ctx, &mut tour, size, clock, Vec::new());
                    }
                    // A second later Mira has finished fading in.
                    clock += 1.0;
                    let (_, window) = frame_at(&ctx, &mut tour, size, clock, Vec::new());
                    assert!(
                        !tour.overflowed,
                        "page {page} tip {tip} scrolls at {size:?}"
                    );
                    assert!(
                        Rect::from_min_size(Pos2::ZERO, size).contains_rect(window),
                        "page {page} leaves the {size:?} window: {window:?}"
                    );
                    // The window keeps one size, so its buttons never move.
                    let expected = *window_size.get_or_insert(window.size());
                    assert!((window.size() - expected).length() < 0.5, "{window:?}");
                }
            }
        }
    }

    #[test]
    fn mira_banner_decodes_with_a_transparent_background() {
        let image = image::load_from_memory_with_format(BANNER, image::ImageFormat::WebP)
            .unwrap()
            .into_rgba8();
        assert_eq!(image.get_pixel(0, 0)[3], 0, "the banner has a backdrop");
        // Close to sharp at twice the header's height on high-DPI screens.
        assert!(image.height() >= 240, "{}", image.height());
        assert!(load_art(&egui::Context::default(), "banner", BANNER).is_some());
    }

    #[test]
    fn mira_spout_sheet_matches_the_layout_the_demos_expect() {
        let image = image::load_from_memory_with_format(SPOUT, image::ImageFormat::WebP)
            .unwrap()
            .into_rgba8();
        // `demos::mira`: a 384×512 drawing on a transparent background, then
        // two opaque face crops stacked to its right.
        assert_eq!(image.dimensions(), (579, 512));
        assert_eq!(image.get_pixel(0, 0)[3], 0, "the drawing has a backdrop");
        assert_eq!(
            image.get_pixel(390, 100)[3],
            0,
            "no gutter after the drawing"
        );
        for top in [16, 16 + 157] {
            for (x, y) in [(416, top), (416 + 146, top + 108)] {
                assert!(image.get_pixel(x, y)[3] > 240, "face crop at {x},{y}");
            }
        }
        assert!(load_art(&egui::Context::default(), "spout", SPOUT).is_some());
    }

    #[test]
    fn arrow_keys_turn_pages_and_the_last_page_or_escape_close_the_tour() {
        let ctx = egui::Context::default();
        let size = Vec2::new(1280.0, 800.0);
        let mut tour = WelcomeTour::default();
        let (close, _) = frame(&ctx, &mut tour, size, vec![key(Key::ArrowLeft)]);
        assert!(!close);
        assert_eq!(tour.page, 0);
        for expected in 1..PAGES.len() {
            let (close, _) = frame(&ctx, &mut tour, size, vec![key(Key::ArrowRight)]);
            assert!(!close);
            assert_eq!(tour.page, expected);
        }
        let (close, _) = frame(&ctx, &mut tour, size, vec![key(Key::ArrowLeft)]);
        assert!(!close);
        assert_eq!(tour.page, PAGES.len() - 2);
        let (close, _) = frame(&ctx, &mut tour, size, vec![key(Key::Enter)]);
        assert!(!close);
        let (close, _) = frame(&ctx, &mut tour, size, vec![key(Key::Enter)]);
        assert!(close);

        let mut tour = WelcomeTour::default();
        let (close, _) = frame(&ctx, &mut tour, size, vec![key(Key::Escape)]);
        assert!(close);
    }

    #[test]
    fn tips_advance_after_their_animation_plays_unless_one_is_chosen() {
        let ctx = egui::Context::default();
        let size = Vec2::new(1280.0, 800.0);
        let after = f64::from(LOOP * PLAYS) + 0.1;
        let mut tour = WelcomeTour::default();
        frame_at(&ctx, &mut tour, size, 10.0, Vec::new());
        frame_at(&ctx, &mut tour, size, 10.0 + after - 0.2, Vec::new());
        assert_eq!(tour.tip, 0);
        frame_at(&ctx, &mut tour, size, 10.0 + after, Vec::new());
        assert_eq!(tour.tip, 1);

        // Choosing a tip, by key or click, keeps it playing.
        let start = 10.0 + after;
        frame_at(&ctx, &mut tour, size, start, vec![key(Key::ArrowDown)]);
        assert_eq!(tour.tip, 2);
        frame_at(&ctx, &mut tour, size, start + after * 3.0, Vec::new());
        assert_eq!(tour.tip, 2);

        // A new page starts again from its first tip.
        let later = start + after * 3.0;
        frame_at(&ctx, &mut tour, size, later, vec![key(Key::ArrowRight)]);
        assert_eq!((tour.page, tour.tip, tour.chosen), (1, 0, false));
    }

    #[test]
    fn every_demo_belongs_to_one_tip_and_draws_through_its_loop() {
        let used: Vec<_> = PAGES
            .iter()
            .flat_map(|page| page.tips.iter().map(|tip| tip.demo))
            .collect();
        assert_eq!(used, Demo::ALL);

        let ctx = egui::Context::default();
        let bindings = HotkeyBindings::default();
        let spout = load_art(&ctx, "spout", SPOUT).unwrap();
        let area = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(400.0, 300.0));
        for demo in Demo::ALL {
            for step in 0..=40 {
                let time = step as f32 * demo.length() / 40.0;
                let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let texture = Some(spout.id());
                        demos::paint(ui.painter(), area, demo, time, &bindings, texture);
                    });
                });
                assert!(!output.shapes.is_empty(), "{demo:?} at {time}");
            }
        }
    }

    #[test]
    fn pages_have_distinct_titles_and_a_handful_of_tips() {
        let mut titles: Vec<_> = PAGES.iter().map(|page| page.title).collect();
        titles.sort_unstable();
        titles.dedup();
        assert_eq!(titles.len(), PAGES.len());
        for page in &PAGES {
            assert!((2..=5).contains(&page.tips.len()), "{}", page.title);
            assert!(page
                .tips
                .iter()
                .all(|tip| !tip.title.is_empty() && !tip.text.is_empty()));
        }
    }
}
