use eframe::egui::{Context, ViewportCommand, WindowLevel};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowAction {
    ToggleAlwaysOnTop,
    SetAlwaysOnTop(bool),
    ToggleClickThrough,
    Show,
}

/// App-wide window preferences. Click-through is deliberately session-only.
pub(crate) struct WindowControls {
    pub always_on_top: bool,
    pub click_through: bool,
    applied: Option<(bool, bool)>,
}

impl WindowControls {
    pub fn new(always_on_top: bool) -> Self {
        Self {
            always_on_top,
            click_through: false,
            applied: None,
        }
    }

    pub fn handle(&mut self, action: WindowAction) {
        match action {
            WindowAction::ToggleAlwaysOnTop => self.always_on_top = !self.always_on_top,
            WindowAction::SetAlwaysOnTop(value) => self.always_on_top = value,
            WindowAction::ToggleClickThrough => self.click_through = !self.click_through,
            // Show doubles as a recovery action from the tray.
            WindowAction::Show => self.click_through = false,
        }
    }

    pub fn filter_input(&self, input: &mut eframe::egui::RawInput) {
        if self.click_through {
            input.events.clear();
            input.modifiers = Default::default();
            input.dropped_files.clear();
            input.hovered_files.clear();
        }
    }

    /// Only issue native window commands when a setting changes.
    pub fn apply(&mut self, ctx: &Context) -> bool {
        let state = (self.always_on_top, self.click_through);
        if self.applied == Some(state) {
            return false;
        }
        if self.applied.map(|state| state.0) != Some(self.always_on_top) {
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(if self.always_on_top {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            }));
        }
        if self.applied.map(|state| state.1) != Some(self.click_through) {
            ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(self.click_through));
        }
        self.applied = Some(state);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(state: &mut WindowControls, ctx: &Context) -> Vec<ViewportCommand> {
        let output = ctx.run(Default::default(), |ctx| {
            state.apply(ctx);
        });
        output.viewport_output[&eframe::egui::ViewportId::ROOT]
            .commands
            .clone()
    }

    #[test]
    fn startup_restores_topmost_but_always_accepts_clicks() {
        let mut state = WindowControls::new(true);
        let ctx = Context::default();
        let output = commands(&mut state, &ctx);
        assert!(output.contains(&ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop)));
        assert!(output.contains(&ViewportCommand::MousePassthrough(false)));
        assert!(commands(&mut state, &ctx).is_empty());
    }

    #[test]
    fn canvas_and_tray_changes_share_state_and_show_restores_input() {
        let mut state = WindowControls::new(false);
        let ctx = Context::default();
        commands(&mut state, &ctx);
        state.handle(WindowAction::SetAlwaysOnTop(true));
        assert_eq!(
            commands(&mut state, &ctx),
            vec![ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop)]
        );
        state.handle(WindowAction::ToggleClickThrough);
        assert_eq!(
            commands(&mut state, &ctx),
            vec![ViewportCommand::MousePassthrough(true)]
        );
        state.handle(WindowAction::ToggleAlwaysOnTop);
        assert_eq!(
            commands(&mut state, &ctx),
            vec![ViewportCommand::WindowLevel(WindowLevel::Normal)]
        );
        assert!(state.click_through);
        state.handle(WindowAction::Show);
        assert_eq!(
            commands(&mut state, &ctx),
            vec![ViewportCommand::MousePassthrough(false)]
        );
        state.handle(WindowAction::ToggleClickThrough);
        state.handle(WindowAction::ToggleClickThrough);
        assert!(commands(&mut state, &ctx).is_empty());
    }

    #[test]
    fn click_through_filters_input_but_keeps_window_lifecycle_events() {
        use eframe::egui::{Event, RawInput, ViewportEvent, ViewportId};
        let mut input = RawInput::default();
        input.events.push(Event::Text("typing".into()));
        input
            .viewports
            .get_mut(&ViewportId::ROOT)
            .unwrap()
            .events
            .push(ViewportEvent::Close);
        let mut state = WindowControls::new(false);
        state.filter_input(&mut input);
        assert_eq!(input.events.len(), 1);
        state.handle(WindowAction::ToggleClickThrough);
        state.filter_input(&mut input);
        assert!(input.events.is_empty());
        assert!(input.viewports[&ViewportId::ROOT]
            .events
            .contains(&ViewportEvent::Close));
    }
}
