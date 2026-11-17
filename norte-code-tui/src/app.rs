use std::{
    io,
    time::{Duration, Instant},
};

use crossterm::event::{self, Event, KeyEventKind, MouseEvent};
use thiserror::Error;
mod actions;
mod client_data;
mod client_events;
mod jobs;
mod render;
mod session_state;

use crate::{
    client::{ClientRuntime, ClientRuntimeError},
    components::{
        Component, ComponentId, HeaderModal, LifecycleComponent, LifecycleEvent,
        LifecycleTransitionError, PlanReviewView, create_views,
    },
    config::{ClientConfig, ConfigError},
    keys,
    terminal::TerminalSession,
    ui_model::UiModel,
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Overlay {
    #[default]
    Closed,
    Help,
}

/// A floating menu a header segment opened, with the cursor on the choice.
#[derive(Clone)]
struct HeaderSelectState {
    kind: HeaderModal,
    selected: usize,
    options: Vec<&'static str>,
    current: usize,
}

impl HeaderSelectState {
    fn new(
        kind: HeaderModal,
        model: &UiModel,
        capabilities: Option<&norte_code_wire::ReasoningInfo>,
    ) -> Self {
        let options = kind.options_for(capabilities);
        let label = match kind {
            HeaderModal::Queue => match model.send_mode {
                crate::ui_model::SendMode::Steer => "steer",
                crate::ui_model::SendMode::Queue => "queue",
                crate::ui_model::SendMode::Interrupt => "interrupt",
                crate::ui_model::SendMode::Reject => "reject",
            },
            HeaderModal::Reasoning => model.reasoning.label(),
        };
        let current = options
            .iter()
            .position(|option| *option == label)
            .unwrap_or(0);
        Self {
            kind,
            selected: current,
            options,
            current,
        }
    }
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
    /// The floating menu a header segment opened.
    header_select: Option<HeaderSelectState>,
    run_state: RunState,
    client: ClientRuntime,
    config: ClientConfig,
    plan_review: PlanReviewView,
    session_generation: u64,
    /// The offset used to scroll the shortcut ticker across the footer.
    ticker_offset: usize,
    /// The last time the shortcut ticker moved.
    last_ticker_tick: Instant,
    /// A refusal the client is already lowering for a chosen effort.
    reasoning_recovery: crate::ui_model::ReasoningRecovery,
    last_session_refresh: Instant,
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
            active_view: ComponentId::Sessions,
            overlay: Overlay::Closed,
            header_select: None,
            run_state: RunState::Running,
            client,
            config,
            plan_review: PlanReviewView::default(),
            session_generation: 0,
            ticker_offset: 0,
            last_ticker_tick: Instant::now(),
            reasoning_recovery: crate::ui_model::ReasoningRecovery::default(),
            last_session_refresh: Instant::now(),
        };
        app.activate_view(ComponentId::Sessions)?;
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
            if self.last_ticker_tick.elapsed() >= Duration::from_millis(350) {
                self.ticker_offset = self.ticker_offset.wrapping_add(1);
                self.last_ticker_tick = Instant::now();
            }
            self.drain_client_events()?;
            if self.model.preferences.auto_refresh_sessions.is_on()
                && self.last_session_refresh.elapsed() >= Duration::from_secs(15)
            {
                self.client.refresh_sessions(self.session_generation);
                self.last_session_refresh = Instant::now();
            }
            terminal.draw(|frame| self.render(frame))?;
            if stop.was_asked() {
                break;
            }
            let (keys, mice) = Self::read_events()?;
            if !keys.is_empty() {
                self.handle_keys(keys)?;
            }
            if !mice.is_empty() {
                self.handle_mouse(&mice)?;
            }
        }
        Ok(())
    }

    /// Reads the keys and the mouse events that are waiting.
    ///
    /// A paste arrives as a burst of keys, and the burst is what tells the
    /// reader that the text was pasted instead of typed.
    fn read_events() -> Result<(Vec<crossterm::event::KeyEvent>, Vec<MouseEvent>), AppError> {
        let mut keys = Vec::new();
        let mut mice = Vec::new();
        if !event::poll(Duration::from_millis(100))? {
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
        if self.plan_review.is_open() || self.overlay == Overlay::Help {
            return Ok(());
        }
        for mouse in mice {
            if mouse.kind == event::MouseEventKind::Down(event::MouseButton::Left) && mouse.row == 0
            {
                let layout = crate::text_builder::TextBuilder::header_layout(&self.model);
                let column = usize::from(mouse.column);
                if let Some((_, _, kind)) = layout
                    .buttons
                    .iter()
                    .find(|(start, end, _)| column >= *start && column < *end)
                {
                    self.header_select = Some(HeaderSelectState::new(
                        *kind,
                        &self.model,
                        self.catalog_capabilities(),
                    ));
                    return Ok(());
                }
            }
        }
        let active_index = self
            .view_index(self.active_view)
            .ok_or(AppError::ComponentNotFound(self.active_view))?;
        self.handle_component_mouse(active_index, mice)?;
        Ok(())
    }

    fn handle_header_select_key(&mut self, key: event::KeyEvent) -> Result<(), AppError> {
        let Some(mut select) = self.header_select.take() else {
            return Ok(());
        };
        let options = select.options.len();
        match key.code {
            keys::CANCEL => {}
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                select.selected = select.selected.saturating_sub(1);
                self.header_select = Some(select);
            }
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                select.selected = (select.selected + 1).min(options.saturating_sub(1));
                self.header_select = Some(select);
            }
            keys::SUBMIT => {
                if let Some(action) = select.kind.action(select.selected, &select.options) {
                    self.header_select = None;
                    self.handle_component_action(action)?;
                    return Ok(());
                }
                self.header_select = Some(select);
            }
            _ => self.header_select = Some(select),
        }
        Ok(())
    }

    fn handle_keys(&mut self, keys: Vec<event::KeyEvent>) -> Result<(), AppError> {
        if self.header_select.is_some() {
            for key in keys {
                self.handle_header_select_key(key)?;
            }
            return Ok(());
        }
        if self.plan_review.is_open() {
            for key in keys {
                self.plan_review.handle_key(key, &self.model);
            }
            if let Some(action) = self.plan_review.take_action() {
                self.handle_component_action(action)?;
            }
            return Ok(());
        }
        let active_index = self
            .view_index(self.active_view)
            .ok_or(AppError::ComponentNotFound(self.active_view))?;
        if self
            .views
            .get(active_index)
            .ok_or(AppError::ComponentNotFound(self.active_view))?
            .captures_global_keys()
        {
            self.handle_component_keys(active_index, &keys)?;
            return Ok(());
        }
        if self.active_view == ComponentId::Sessions
            && self
                .views
                .get(self.view_index(ComponentId::Sessions).unwrap_or(usize::MAX))
                .is_some_and(|view| view.captures_global_keys())
        {
            self.handle_component_keys(active_index, &keys)?;
            return Ok(());
        }
        if self.overlay == Overlay::Help {
            if keys.iter().any(|key| key.code == keys::CANCEL) {
                self.overlay = Overlay::Closed;
            }
            return Ok(());
        }
        if keys.iter().any(|key| key.code == keys::QUIT) {
            self.run_state = RunState::Quit;
            return Ok(());
        }
        if keys.iter().any(|key| key.code == keys::HELP) {
            self.overlay = Overlay::Help;
            return Ok(());
        }
        for key in &keys {
            let code = key.code;
            if code == crossterm::event::KeyCode::Char(keys::SESSIONS_VIEW_CHAR) {
                self.activate_view(ComponentId::Sessions)?;
                return Ok(());
            }
            if let crossterm::event::KeyCode::Char(number) = code
                && let Some(view) = ComponentId::from_shortcut(number)
            {
                self.activate_view(view)?;
                return Ok(());
            }
        }
        self.handle_component_keys(active_index, &keys)?;
        Ok(())
    }

    fn handle_component_keys(
        &mut self,
        active_index: usize,
        keys: &[event::KeyEvent],
    ) -> Result<(), AppError> {
        self.route_to_component(active_index, |component, model| {
            component.handle_keys(keys, model);
        })
    }

    fn handle_component_mouse(
        &mut self,
        active_index: usize,
        mice: &[MouseEvent],
    ) -> Result<(), AppError> {
        self.route_to_component(active_index, |component, model| {
            for mouse in mice {
                component.handle_mouse(*mouse, model);
            }
        })
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

    pub(super) fn activate_view(&mut self, view: ComponentId) -> Result<(), AppError> {
        let target_index = self
            .view_index(view)
            .ok_or(AppError::ComponentNotFound(view))?;
        if self.active_view == view
            && self
                .views
                .get(target_index)
                .ok_or(AppError::ComponentNotFound(view))?
                .lifecycle()
                .phase()
                != crate::components::LifecyclePhase::Dormant
        {
            return Ok(());
        }
        let active_index = self
            .view_index(self.active_view)
            .ok_or(AppError::ComponentNotFound(self.active_view))?;
        if self
            .views
            .get(active_index)
            .ok_or(AppError::ComponentNotFound(self.active_view))?
            .lifecycle()
            .phase()
            != crate::components::LifecyclePhase::Dormant
        {
            self.views
                .get_mut(active_index)
                .ok_or(AppError::ComponentNotFound(self.active_view))?
                .transition(LifecycleEvent::Unmount)?;
        }
        self.active_view = view;
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

    fn view_has_data(&self, view: ComponentId) -> bool {
        match view {
            ComponentId::Session => self.model.active_session.is_some(),
            ComponentId::Plans => !self.model.plans.is_empty(),
            ComponentId::Telemetry => self.model.telemetry.is_some(),
            ComponentId::Plugins => !self.model.plugins.is_empty(),
            ComponentId::Models => !self.model.models.is_empty(),
            ComponentId::Settings => true,
            ComponentId::Host => self.model.server.is_some(),
            ComponentId::Sessions => {
                !self.model.sessions.is_empty() || !self.model.children.is_empty()
            }
            ComponentId::Jobs => !self.model.jobs.is_empty(),
            ComponentId::Costs => self.model.costs.is_some() || self.model.session_costs.is_some(),
            ComponentId::PlanReview => self.plan_review.is_open(),
        }
    }

    fn view_index(&self, id: ComponentId) -> Option<usize> {
        self.views.iter().position(|view| view.id() == id)
    }
}
