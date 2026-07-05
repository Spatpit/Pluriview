# Add Browser UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users add and remove multiple browser tiles from the canvas by pasting any HTTP(S) website URL.

**Architecture:** Replace the one-off spike host with a `HashMap<PreviewId, BrowserHost>`. The canvas requests an Add Browser dialog at the clicked canvas position; the app validates the URL, creates the host and preview, and uses the existing capture and double-click interaction paths.

**Tech Stack:** Rust, egui, Wry/WebView2, Windows Graphics Capture.

---

### Task 1: Normalize browser URLs

**Files:**
- Modify: `src/browser.rs`

- [ ] Add failing tests proving `twitch.tv/name` becomes `https://twitch.tv/name`, HTTP(S) remains unchanged, and `file:`/`javascript:` are rejected.
- [ ] Run `cargo test normalize_url` and verify the missing function fails compilation.
- [ ] Add this minimal API:

```rust
pub fn normalize_url(input: &str) -> Result<String, &'static str> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter a website URL");
    }
    if input.starts_with("http://") || input.starts_with("https://") {
        return Ok(input.to_owned());
    }
    if input.contains(':') {
        return Err("Only HTTP and HTTPS websites are supported");
    }
    Ok(format!("https://{input}"))
}
```

- [ ] Run `cargo test normalize_url`; expected: all URL tests pass.

### Task 2: Request a browser from the canvas

**Files:**
- Modify: `src/canvas/state.rs`

- [ ] Add a failing default-state test for `pending_browser_add: Option<Pos2>`.
- [ ] Add `pending_browser_add` to `CanvasState`, initialized to `None`.
- [ ] Add **Add Browser…** beside **Add Window…** in the empty-canvas context menu; set `pending_browser_add` to the clicked canvas position.
- [ ] Run `cargo test`; expected: all tests pass.

### Task 3: Add multiple browser hosts and URL dialog

**Files:**
- Modify: `src/app.rs`

- [ ] Replace `browser_spike: Option<(PreviewId, BrowserHost)>` with:

```rust
browser_hosts: HashMap<PreviewId, BrowserHost>,
add_browser: Option<(Pos2, String, Option<String>)>,
```

- [ ] Add `create_browser(&mut self, url: &str, position: Pos2) -> Result<(), String>` that normalizes the URL, creates `BrowserHost`, adds a 640×360 preview, starts 30 FPS capture, and inserts the host by preview ID.
- [ ] Consume `canvas.pending_browser_add` after canvas rendering and open an egui window containing one URL text field plus **Add** and **Cancel** buttons.
- [ ] On Add or Enter, call `create_browser`; keep the dialog open with the returned error or close it on success.
- [ ] Remove the `PLURIVIEW_BROWSER_SPIKE_URL` startup path.
- [ ] Run `cargo test` and `cargo check`; expected: all pass without warnings.

### Task 4: Generalize interaction and cleanup

**Files:**
- Modify: `src/app.rs`

- [ ] When a preview is double-clicked, activate it only if its ID exists in `browser_hosts`; park any other active browser first.
- [ ] When Pluriview regains focus, park every active browser host.
- [ ] Each update, retain only hosts whose preview still exists so removing a tile drops its WebView and HWND.
- [ ] Keep `Ctrl+B` as an optional shortcut for the selected browser tile.
- [ ] Run `cargo test`, `cargo check`, and `cargo build`; expected: all pass.
- [ ] Manually add YouTube, Twitch, and Kick URLs; remove one and add it again; verify double-click interaction and canvas-click parking.
- [ ] Commit with `git commit -m "Add browser tiles from canvas URLs"`.

## Deferred

Persisting browser URLs across app restarts is intentionally excluded from this pass. Add it after the add/remove/interaction flow is accepted.
