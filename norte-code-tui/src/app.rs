use std::{
    io,
    time::{Duration, Instant},
};

use crossterm::event::{self, Event, KeyEventKind, MouseEvent};
use ratatui::layout::Rect;
use thiserror::Error;

mod actions;
mod client_data;
mod client_events;
mod input;
mod jobs;
mod prompt;
mod render;
mod session_state;

use crate::{
    client::{ClientRuntime, ClientRuntimeError},
    components::{
        CommandMenu, ComponentId, LifecycleComponent, LifecycleEvent, LifecycleTransitionError,
        PlanReviewView, create_views,
    },
    config::{ClientConfig, ConfigError},
    terminal::TerminalSession,
    ui_model::{Notice, UiModel},
};

#[derive(Debug, Error)]
pub(crate) enum AppError {
    #[error(transparent)]
    Terminal(#[from] io::Error),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleTransitionError),
    #[error(transparent)]
    Client(#[from] ClientRuntimeError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("component {0:?} is not registered")]
    ComponentNotFound(ComponentId),
}

/// The one overlay drawn over the screen.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Overlay {
    #[default]
    Closed,
    Help,
}

pub(crate) fn run() -> Result<(), AppError> {
    let mut app = App::new()?;
    let mut terminal = TerminalSession::enter()?;
    let result = app.run_loop(terminal.terminal_mut());
    terminal.restore()?;
    result
}

struct App {
    model: UiModel,
    views: Vec<Box<dyn LifecycleComponent>>,
    active_view: ComponentId,
    overlay: Overlay,
    menu: CommandMenu,
    /// The text prompt the menu opened, if one is open.
    prompt: Option<prompt::PromptState>,
    run_state: RunState,
    client: ClientRuntime,
    config: ClientConfig,
    plan_review: PlanReviewView,
    session_generation: u64,
    /// A refusal the client is already lowering for a chosen effort.
    reasoning_recovery: crate::ui_model::ReasoningRecovery,
    last_session_refresh: Instant,
    /// The tick the animations read; it advances only while something moves.
    animation_phase: u16,
    /// Whether the last frame already showed the current state.
    needs_redraw: bool,
    /// The size of the last frame, so a click maps to a tab column.
    frame_area: Rect,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum RunState {
    #[default]
    Running,
    Quit,
}

impl App {
    fn new() -> Result<Self, AppError> {
        let config = ClientConfig::load()?;
        let (client, sessions, fresh) = ClientRuntime::connect(&config)?;
        let mut model = UiModel {
            sessions,
            endpoint: client.endpoint().to_owned(),
            authentication_configured: client.authentication_configured(),
            ..UiModel::default()
        };
        config.apply_to(&mut model);
        let mut app = Self {
            model,
            views: create_views(),
            active_view: ComponentId::Session,
            overlay: Overlay::Closed,
            menu: CommandMenu::default(),
            prompt: None,
            run_state: RunState::Running,
            client,
            config,
            plan_review: PlanReviewView::default(),
            session_generation: 0,
            reasoning_recovery: crate::ui_model::ReasoningRecovery::default(),
            last_session_refresh: Instant::now(),
            animation_phase: 0,
            needs_redraw: true,
            frame_area: Rect::default(),
        };
        app.activate_view(ComponentId::Session)?;
        app.open_session(fresh);
        app.client.load_models(app.session_generation);
        app.client.load_catalog(app.session_generation);
        app.client.load_host(app.session_generation);
        app.client.load_plugins(app.session_generation);
        app.client.load_telemetry(None, app.session_generation);
        app.client.load_costs(None, app.session_generation);
        Ok(app)
    }

    fn run_loop(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<(), AppError> {
        let stop = crate::signals::StopSignal::listen();
        while self.run_state == RunState::Running {
            self.advance_animation();
            self.expire_notice();
            if self.drain_client_events()? {
                self.needs_redraw = true;
            }
            self.refresh_sessions_if_due();
            if self.needs_redraw || self.animating() {
                terminal.draw(|frame| self.render(frame))?;
                self.needs_redraw = false;
            }
            if stop.was_asked() {
                break;
            }
            let (keys, mice) = Self::read_events(self.poll_timeout())?;
            if !keys.is_empty() {
                self.handle_keys(&keys)?;
                self.needs_redraw = true;
            }
            if !mice.is_empty() {
                self.handle_mouse(&mice)?;
                self.needs_redraw = true;
            }
        }
        Ok(())
    }

    /// Moves the sweep one step, only while something is moving.
    fn advance_animation(&mut self) {
        if self.animating() {
            self.animation_phase = self.animation_phase.wrapping_add(1);
        }
    }

    /// How long to wait for input before checking the clock again.
    fn poll_timeout(&self) -> Duration {
        if self.animating() {
            Duration::from_millis(80)
        } else {
            Duration::from_millis(250)
        }
    }

    /// Whether something on screen is moving on its own.
    fn animating(&self) -> bool {
        self.model.turn_state.is_busy()
            || self
                .model
                .notice
                .as_ref()
                .is_some_and(|notice| notice.age() < Duration::from_secs(1))
    }

    fn refresh_sessions_if_due(&mut self) {
        if self.model.preferences.auto_refresh_sessions.is_on()
            && self.last_session_refresh.elapsed() >= Duration::from_secs(15)
        {
            self.client.refresh_sessions(self.session_generation);
            self.last_session_refresh = Instant::now();
        }
    }

    /// Shows a notice as a toast.
    pub(super) fn set_notice(&mut self, notice: Notice) {
        if let Some(current) = &self.model.notice
            && !notice.overrides(current)
        {
            return;
        }
        self.model.notice = Some(notice);
        self.needs_redraw = true;
    }

    /// Drops the notice that is showing.
    pub(super) fn dismiss_notice(&mut self) {
        if self.model.notice.take().is_some() {
            self.needs_redraw = true;
        }
    }

    fn expire_notice(&mut self) {
        if self.model.notice.as_ref().is_some_and(Notice::expired) {
            self.model.notice = None;
            self.needs_redraw = true;
        }
    }

    /// Reads the keys and the mouse events that are waiting.
    ///
    /// A paste arrives as a burst of keys, and the burst is what tells the
    /// reader that the text was pasted instead of typed.
    fn read_events(
        timeout: Duration,
    ) -> Result<(Vec<crossterm::event::KeyEvent>, Vec<MouseEvent>), AppError> {
        let mut keys = Vec::new();
        let mut mice = Vec::new();
        if !event::poll(timeout)? {
            return Ok((keys, mice));
        }
        while let Ok(event) = event::read() {
            match event {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        keys.push(key);
                    }
                }
                Event::Mouse(mouse) => mice.push(mouse),
                _ => {}
            }
            if !event::poll(Duration::ZERO)? {
                break;
            }
        }
        Ok((keys, mice))
    }

    fn handle_mouse(&mut self, mice: &[MouseEvent]) -> Result<(), AppError> {
        if self.modal_open() {
            return Ok(());
        }
        for mouse in mice {
            if mouse.kind == event::MouseEventKind::Down(event::MouseButton::Left) && mouse.row == 0
            {
                let layout = crate::text_builder::TextBuilder::tab_bar(
                    &self.model,
                    self.active_view,
                    self.frame_area.width,
                );
                let column = usize::from(mouse.column);
                if let Some((_, _, tab)) = layout
                    .tabs
                    .iter()
                    .find(|(start, end, _)| column >= *start && column < *end)
                {
                    let tab = *tab;
                    self.activate_view(tab)?;
                    return Ok(());
                }
            }
        }
        let active_index = self
            .view_index(self.active_view)
            .ok_or(AppError::ComponentNotFound(self.active_view))?;
        self.route_to_component(active_index, |component, model| {
            for mouse in mice {
                component.handle_mouse(*mouse, model);
            }
        })?;
        Ok(())
    }

    fn route_to_component(
        &mut self,
        active_index: usize,
        handle: impl FnOnce(&mut dyn LifecycleComponent, &UiModel),
    ) -> Result<(), AppError> {
        let action = {
            let component = self
                .views
                .get_mut(active_index)
                .ok_or(AppError::ComponentNotFound(self.active_view))?;
            handle(component.as_mut(), &self.model);
            component.take_action()
        };
        if let Some(action) = action {
            self.handle_component_action(action)?;
        }
        Ok(())
    }

    fn activate_view(&mut self, view: ComponentId) -> Result<(), AppError> {
        let phase = self.view_phase(view);
        if self.active_view == view && phase != LifecyclePhase::Dormant {
            return Ok(());
        }
        let active_index = self
            .view_index(self.active_view)
            .ok_or(AppError::ComponentNotFound(self.active_view))?;
        if self.view_phase(self.active_view) != LifecyclePhase::Dormant {
            self.views
                .get_mut(active_index)
                .ok_or(AppError::ComponentNotFound(self.active_view))?
                .transition(LifecycleEvent::Unmount)?;
        }
        self.active_view = view;
        let target_index = self
            .view_index(view)
            .ok_or(AppError::ComponentNotFound(view))?;
        let lifecycle_event = if self.view_has_data(view) {
            LifecycleEvent::Loaded
        } else {
            LifecycleEvent::Empty
        };
        let component = self
            .views
            .get_mut(target_index)
            .ok_or(AppError::ComponentNotFound(view))?;
        component.transition(LifecycleEvent::Mount)?;
        component.transition(LifecycleEvent::LoadStarted)?;
        component.transition(lifecycle_event)?;
        Ok(())
    }

    fn view_phase(&self, view: ComponentId) -> LifecyclePhase {
        self.view_index(view)
            .and_then(|index| self.views.get(index))
            .map_or(LifecyclePhase::Dormant, |component| {
                component.lifecycle().phase()
            })
    }

    fn view_has_data(&self, view: ComponentId) -> bool {
        match view {
            ComponentId::Session => self.model.active_session.is_some(),
            ComponentId::Sessions => {
                !self.model.sessions.is_empty() || !self.model.children.is_empty()
            }
            ComponentId::Models => !self.model.models.is_empty(),
            ComponentId::Plans => !self.model.plans.is_empty(),
            ComponentId::Jobs => !self.model.jobs.is_empty(),
            ComponentId::Telemetry => self.model.telemetry.is_some(),
            ComponentId::Host => self.model.server.is_some(),
            ComponentId::Plugins => !self.model.plugins.is_empty(),
            ComponentId::Costs => self.model.costs.is_some() || self.model.session_costs.is_some(),
            ComponentId::PlanReview => self.plan_review.is_open(),
        }
    }

    fn view_index(&self, id: ComponentId) -> Option<usize> {
        self.views.iter().position(|view| view.id() == id)
    }

    /// Whether an overlay owns the keys.
    fn modal_open(&self) -> bool {
        self.overlay == Overlay::Help
            || self.menu.is_open()
            || self.plan_review.is_open()
            || self.prompt.is_some()
    }

    fn set_quit(&mut self) {
        self.run_state = RunState::Quit;
    }

    /// A shell of an application, for the tests that draw one frame.
    #[cfg(test)]
    pub(crate) fn for_test(tab: ComponentId) -> Self {
        let config = ClientConfig::default();
        let client = ClientRuntime::detached(&config).expect("a client needs no host");
        let mut model = UiModel::default();
        config.apply_to(&mut model);
        let mut app = Self {
            model,
            views: create_views(),
            active_view: tab,
            overlay: Overlay::Closed,
            menu: CommandMenu::default(),
            prompt: None,
            run_state: RunState::Running,
            client,
            config,
            plan_review: PlanReviewView::default(),
            session_generation: 0,
            reasoning_recovery: crate::ui_model::ReasoningRecovery::default(),
            last_session_refresh: Instant::now(),
            animation_phase: 0,
            needs_redraw: true,
            frame_area: Rect::default(),
        };
        app.activate_view(tab).expect("the tab mounts");
        app
    }

    /// The overlay the interface is showing, for a test to read.
    #[cfg(test)]
    pub(crate) const fn overlay_for_test(&self) -> Overlay {
        self.overlay
    }
}

#[cfg(test)]
#[path = "app/prompt_tests.rs"]
mod prompt_tests;

use crate::components::LifecyclePhase;
