//! Geometry of a browser tile's toolbar and vertical tab strip.
//!
//! The frame is measured in the tile's own units (canvas units, or screen
//! points for a viewport-pinned tile) and scales with the tile, so zooming
//! the canvas never changes the page's share of the tile and therefore never
//! relayouts the page.

use eframe::egui::{Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};

use super::Preview;

pub const TOOLBAR_HEIGHT: f32 = 36.0;
pub const TAB_STRIP_WIDTH: f32 = 184.0;
pub const TAB_STRIP_COLLAPSED_WIDTH: f32 = 44.0;
/// Below this size the frame would crowd the page, so the page fills the tile.
pub const MIN_TILE_WIDTH: f32 = 360.0;
pub const MIN_TILE_HEIGHT: f32 = 200.0;

/// One page in a browser tile. The URL is scrubbed before it is saved.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BrowserTab {
    pub url: String,
    #[serde(default)]
    pub title: String,
    /// The page is producing sound (runtime only).
    #[serde(skip)]
    pub playing_audio: bool,
}

impl BrowserTab {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Self::default()
        }
    }

    /// The page title, or the address while the page has not named itself.
    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() {
            &self.url
        } else {
            &self.title
        }
    }
}

/// Where the toolbar, tab strip and page sit inside a tile rect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromeLayout {
    pub toolbar: Rect,
    pub tabs: Rect,
    pub page: Rect,
    /// Screen points per tile unit, for sizing text and icons.
    pub scale: f32,
}

impl ChromeLayout {
    pub fn new(tile: Rect, scale: f32, collapsed: bool) -> Self {
        let scale = scale.max(0.01);
        let toolbar_height = (TOOLBAR_HEIGHT * scale).min(tile.height());
        let strip_width = (strip_width(collapsed) * scale).min(tile.width());
        let toolbar = Rect::from_min_size(tile.min, Vec2::new(tile.width(), toolbar_height));
        let tabs = Rect::from_min_max(
            Pos2::new(tile.min.x, toolbar.max.y),
            Pos2::new(tile.min.x + strip_width, tile.max.y),
        );
        let page = Rect::from_min_max(Pos2::new(tabs.max.x, toolbar.max.y), tile.max);
        Self {
            toolbar,
            tabs,
            page,
            scale,
        }
    }
}

fn strip_width(collapsed: bool) -> f32 {
    if collapsed {
        TAB_STRIP_COLLAPSED_WIDTH
    } else {
        TAB_STRIP_WIDTH
    }
}

/// The tile's size in its own units: screen points when pinned, otherwise
/// canvas units.
fn model_size(preview: &Preview) -> Vec2 {
    preview
        .viewport_pin
        .map(|pin| pin.size_vec2())
        .unwrap_or(preview.size)
}

/// Whether `preview` draws its toolbar and tabs right now. A cropped tile
/// shows only the part of the page it was cropped to.
pub fn shows_chrome(preview: &Preview) -> bool {
    let size = model_size(preview);
    preview.is_browser()
        && preview.browser_chrome
        && preview.crop_uv.is_none()
        && size.x >= MIN_TILE_WIDTH
        && size.y >= MIN_TILE_HEIGHT
}

/// The frame layout for `preview` drawn at `tile_screen_rect`, or None when
/// the page fills the tile.
pub fn layout_for(preview: &Preview, tile_screen_rect: Rect) -> Option<ChromeLayout> {
    if !shows_chrome(preview) {
        return None;
    }
    let size = model_size(preview);
    let scale = tile_screen_rect.width() / size.x.max(1.0);
    Some(ChromeLayout::new(
        tile_screen_rect,
        scale,
        preview.browser_tabs_collapsed,
    ))
}

/// The part of `tile_screen_rect` the page occupies.
pub fn page_rect_for(preview: &Preview, tile_screen_rect: Rect) -> Rect {
    layout_for(preview, tile_screen_rect).map_or(tile_screen_rect, |layout| layout.page)
}

/// The page's rect inside a tile rect given in the tile's own units.
pub fn strip_chrome(tile: Rect, collapsed: bool) -> Rect {
    ChromeLayout::new(tile, 1.0, collapsed).page
}

/// The tile rect that frames a page rect given in the tile's own units.
pub fn wrap_with_chrome(page: Rect, collapsed: bool) -> Rect {
    Rect::from_min_max(
        page.min - Vec2::new(strip_width(collapsed), TOOLBAR_HEIGHT),
        page.max,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_sits_right_of_the_tabs_and_below_the_toolbar() {
        let tile = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(800.0, 500.0));
        let layout = ChromeLayout::new(tile, 1.0, false);
        assert_eq!(layout.toolbar.height(), TOOLBAR_HEIGHT);
        assert_eq!(layout.tabs.width(), TAB_STRIP_WIDTH);
        assert_eq!(
            layout.page,
            Rect::from_min_max(
                Pos2::new(10.0 + TAB_STRIP_WIDTH, 20.0 + TOOLBAR_HEIGHT),
                tile.max
            )
        );
    }

    #[test]
    fn frame_scales_with_the_tile_so_the_page_share_is_stable() {
        let tile = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 500.0));
        let near = ChromeLayout::new(tile, 1.0, false).page;
        let far = ChromeLayout::new(
            Rect::from_min_size(Pos2::ZERO, tile.size() * 0.5),
            0.5,
            false,
        )
        .page;
        assert!((near.width() * 0.5 - far.width()).abs() < 1e-3);
        assert!((near.height() * 0.5 - far.height()).abs() < 1e-3);
    }

    #[test]
    fn wrapping_restores_the_tile_that_was_stripped() {
        let tile = Rect::from_min_size(Pos2::new(-40.0, 15.0), Vec2::new(900.0, 560.0));
        for collapsed in [false, true] {
            let page = strip_chrome(tile, collapsed);
            let wrapped = wrap_with_chrome(page, collapsed);
            assert!((wrapped.min - tile.min).length() < 1e-3);
            assert!((wrapped.max - tile.max).length() < 1e-3);
        }
    }

    #[test]
    fn small_or_cropped_tiles_have_no_frame() {
        let mut preview = Preview::new(
            super::super::PreviewId(1),
            "browser".to_owned(),
            Pos2::ZERO,
            Vec2::new(900.0, 560.0),
        );
        preview.browser_url = Some("https://example.com".to_owned());
        preview.browser_chrome = true;
        assert!(shows_chrome(&preview));
        preview.crop_uv = Some((0.0, 0.0, 0.5, 0.5));
        assert!(!shows_chrome(&preview));
        preview.crop_uv = None;
        preview.size = Vec2::new(300.0, 560.0);
        assert!(!shows_chrome(&preview));
    }

    #[test]
    fn untitled_tabs_are_labelled_by_their_address() {
        let mut tab = BrowserTab::new("https://example.com/");
        assert_eq!(tab.label(), "https://example.com/");
        tab.title = "Example".to_owned();
        assert_eq!(tab.label(), "Example");
    }
}
