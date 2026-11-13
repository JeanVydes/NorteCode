use std::{
    io,
    time::{Duration, Instant},
};

use crossterm::event::{self, Event, KeyEventKind};
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
        Component, ComponentId, LifecycleComponent, LifecycleEvent, LifecycleTransitionError,
        PlanReviewView, create_views,
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
    run_state: RunState,
    client: ClientRuntime,
    config: ClientConfig,
    plan_review: PlanReviewView,
    session_generation: u64,
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
            run_state: RunState::Running,
            client,
            config,
            plan_review: PlanReviewView::default(),
            session_generation: 0,
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
            let keys = Self::read_keys()?;
            if !keys.is_empty() {
                self.handle_keys(keys)?;
            }
        }
        Ok(())
    }

    /// Reads the keys that are waiting, the way a terminal delivers them.
    ///
    /// A paste arrives as a burst of keys, and the burst is what tells the
    /// reader that the text was pasted instead of typed.
    fn read_keys() -> Result<Vec<crossterm::event::KeyEvent>, AppError> {
        let mut keys = Vec::new();
        if !event::poll(Duration::from_millis(100))? {
            return Ok(keys);
        }
        while let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                keys.push(key);
            }
            if !event::poll(Duration::ZERO)? {
                break;
            }
        }
        Ok(keys)
    }

    fn handle_keys(&mut self, keys: Vec<event::KeyEvent>) -> Result<(), AppError> {
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
            && let Some(event::KeyCode::Char('1'..='9')) = keys.first().map(|key| key.code)
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
        let action = {
            let component = self
                .views
                .get_mut(active_index)
                .ok_or(AppError::ComponentNotFound(self.active_view))?;
            component.handle_keys(keys, &self.model);
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
            ComponentId::Catalog => {
                !self.model.catalog.providers.is_empty() || !self.model.catalog.models.is_empty()
            }
            ComponentId::Settings => true,
            ComponentId::Host => self.model.server.is_some(),
            ComponentId::Sessions => {
                !self.model.sessions.is_empty() || !self.model.children.is_empty()
            }
            ComponentId::Costs => self.model.costs.is_some() || self.model.session_costs.is_some(),
            ComponentId::PlanReview => self.plan_review.is_open(),
        }
    }

    fn view_index(&self, id: ComponentId) -> Option<usize> {
        self.views.iter().position(|view| view.id() == id)
    }
}
