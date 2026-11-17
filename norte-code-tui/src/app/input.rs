//! How a key finds its owner.
//!
//! The layers are checked in one order and each one either consumes the keys or
//! passes them down: an overlay, the view that owns the keys, the global
//! shortcuts. That order is what lets `Esc` mean "leave the composer", then
//! "open the menu", then "close the menu".
//!
//! A burst that no layer claims is handed to the view as one slice, because a
//! terminal delivers a paste as a burst and only the slice tells it apart from
//! typing.

use crossterm::event::KeyEvent;

use super::{App, AppError, Overlay};
use crate::components::{CommandOutcome, ComponentId};
use crate::keys;

impl App {
    pub(super) fn handle_keys(&mut self, keys: &[KeyEvent]) -> Result<(), AppError> {
        if self.handle_overlay_keys(keys)? {
            return Ok(());
        }
        if self.handle_view_owned_keys(keys)? {
            return Ok(());
        }
        if self.handle_global_keys(keys)? {
            return Ok(());
        }
        self.forward_to_view(keys)
    }

    /// The overlays: help, the command menu and the plan review.
    fn handle_overlay_keys(&mut self, keys: &[KeyEvent]) -> Result<bool, AppError> {
        if self.plan_review.is_open() {
            for key in keys {
                self.plan_review.key(*key, &self.model);
            }
            if let Some(action) = self.plan_review.action() {
                self.handle_component_action(action)?;
            }
            return Ok(true);
        }
        if self.prompt_open() {
            for key in keys {
                self.prompt_key(*key)?;
            }
            return Ok(true);
        }
        if self.menu.is_open() {
            for key in keys {
                if let CommandOutcome::Run(action) = self.menu.handle_key(*key) {
                    self.handle_menu_action(action);
                }
            }
            return Ok(true);
        }
        if self.overlay == Overlay::Help {
            if keys
                .iter()
                .any(|key| key.code == keys::CANCEL || key.code == keys::HELP)
            {
                self.overlay = Overlay::Closed;
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// The keys the open view owns while it has a prompt, a filter or a composer.
    fn handle_view_owned_keys(&mut self, keys: &[KeyEvent]) -> Result<bool, AppError> {
        let Some(index) = self.view_index(self.active_view) else {
            return Ok(false);
        };
        let captures = self
            .views
            .get(index)
            .is_some_and(|component| component.captures_global_keys());
        if !captures {
            return Ok(false);
        }
        self.route_to_component(index, |component, model| {
            component.handle_keys(keys, model);
        })?;
        Ok(true)
    }

    /// The keys that work from anywhere, when no view owns them.
    ///
    /// A burst of plain characters is not a shortcut: it is pasted text, and it
    /// belongs to the view.
    fn handle_global_keys(&mut self, keys: &[KeyEvent]) -> Result<bool, AppError> {
        if keys.len() > 1 && keys.iter().all(|key| key.modifiers.is_empty()) {
            return Ok(false);
        }
        for key in keys {
            match key.code {
                keys::QUIT => {
                    self.set_quit();
                    return Ok(true);
                }
                keys::HELP => {
                    self.overlay = Overlay::Help;
                    return Ok(true);
                }
                keys::MENU => {
                    if self.model.notice.is_some() {
                        self.dismiss_notice();
                    } else {
                        self.menu.open(&self.model);
                    }
                    return Ok(true);
                }
                keys::NEXT_TAB => {
                    let next = self.active_view.next_tab();
                    self.activate_view(next)?;
                    return Ok(true);
                }
                keys::PREV_TAB => {
                    let previous = self.active_view.previous_tab();
                    self.activate_view(previous)?;
                    return Ok(true);
                }
                crossterm::event::KeyCode::Char(digit) => {
                    if let Some(view) = ComponentId::from_shortcut(digit) {
                        self.activate_view(view)?;
                        return Ok(true);
                    }
                }
                _ => {}
            }
        }
        Ok(false)
    }

    /// The burst goes to the view that is showing, as one unit.
    fn forward_to_view(&mut self, keys: &[KeyEvent]) -> Result<(), AppError> {
        let index = self
            .view_index(self.active_view)
            .ok_or(AppError::ComponentNotFound(self.active_view))?;
        self.route_to_component(index, |component, model| {
            component.handle_keys(keys, model);
        })
    }
}
