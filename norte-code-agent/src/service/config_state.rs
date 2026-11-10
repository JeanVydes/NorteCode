//! One of the service's domains, as `impl Service` blocks.
//!
//! Child modules see the service's private fields, so a domain owns its
//! behaviour here while the state stays in one place.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError};

use norte_code_protocol::catalog::{Catalog, load_catalog_files};
use norte_code_protocol::config::{ApiKey, Config};
use norte_code_protocol::keys::KeyStore;
use norte_code_wire::SessionKey;

use crate::ServiceError;
use crate::provider::ModelSpec;

use skills::{collect_secrets, discover_skills};

#[allow(clippy::wildcard_imports)]
use super::*;

impl Service {
    /// Selects the config file global approval rules are written to.
    ///
    /// The CLI calls this after loading `--config`, so "always" lands in the
    /// file the user actually named. Without it, the platform user config path
    /// is used.
    pub fn set_config_path(&self, path: Option<PathBuf>) {
        *self
            .config_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = path;
    }
    /// The config file the process was pointed at, if any.
    ///
    /// Kept here rather than threaded through every caller, so a report can say
    /// which file is in force. `None` means the platform default path is used.
    #[must_use]
    pub fn config_path(&self) -> Option<PathBuf> {
        self.config_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    /// The configuration in force.
    ///
    /// A value rather than a borrow: the config is held behind a lock so a
    /// reload can replace it, and it is a small type of strings and numbers.
    /// The lock is held only long enough to copy.
    #[must_use]
    pub fn config(&self) -> Config {
        self.lock_config().clone()
    }
    /// The key store the host installed, when it did.
    ///
    /// A session asks it before switching to a provider that has no key, so the
    /// client can prompt for one and save it.
    #[must_use]
    pub fn key_store(&self) -> Option<Arc<dyn KeyStore>> {
        self.lock_config().keys.clone()
    }
    /// Adds a key resolved after startup to the redaction list.
    ///
    /// A provider selected through `/model` never passed `collect_secrets` at
    /// construction, and its key must still be hidden from a tool's output.
    pub(super) fn remember_secret(&self, key: &ApiKey) {
        let value = key.expose().trim();
        if value.is_empty() {
            return;
        }
        let mut secrets = self
            .secrets
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if secrets.iter().any(|secret| secret == value) {
            return;
        }
        secrets.push(value.to_owned());
        secrets.sort();
    }
    /// Re-reads the configuration and rebuilds what it derived.
    ///
    /// Returns the new configuration so the caller can report what changed.
    ///
    /// Rebuilt: the default spec, the secrets registered for redaction, the
    /// skill registry and the global approval rules. Not rebuilt: session state.
    /// A session that already resolved a model keeps it, and the rules the user
    /// granted during this run stay granted; the new default applies to the next
    /// session.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Config`] when the file cannot be read, parsed or
    /// validated, and [`ServiceError::Provider`] when the default model it names
    /// cannot be built. Nothing is replaced in either case.
    pub fn reload_config(&self) -> Result<ReloadedConfig, ServiceError> {
        let path = self
            .config_path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let project = self
            .workspace_root
            .ancestors()
            .map(|dir| dir.join(".norte-code").join("service.json"))
            .find(|candidate| candidate.exists());
        let current_agent = self.lock_config().name.clone();
        let service =
            norte_code_protocol::config::ServiceConfig::load(path.as_deref(), project.as_deref())?;
        let agent_profiles: BTreeMap<String, Config> = service
            .agents
            .iter()
            .cloned()
            .map(|agent| (agent.name.clone(), agent))
            .collect();
        let mut warnings = Vec::new();
        let config = service
            .agents
            .iter()
            .find(|agent| agent.name == current_agent)
            .cloned()
            .or_else(|| service.default_agent().ok())
            .ok_or_else(|| {
                ServiceError::Config(norte_code_protocol::config::ConfigError::Invalid {
                    path: "agents".to_owned(),
                    message: "no configured agent is available".to_owned(),
                })
            })?;
        // The file never carries the store; the host installed it on the live
        // config, so a reload has to carry it across or a keyring key would
        // disappear from every session the reload rebuilds.
        let mut next = config;
        next.keys.clone_from(&self.lock_config().keys);

        // Everything that can fail happens before the live service is touched.
        // The catalogue is re-read from disk, so a provider or a model added to
        // providers.json / models.json is visible at once: /reload picks up new
        // providers and models without restarting the application.
        let catalog = match load_catalog_files() {
            Ok(catalog) => {
                *self.lock_catalog_write() = Arc::new(catalog);
                self.catalog()
            }
            // A malformed catalogue file must not abort the whole reload:
            // the catalogue already in memory stays in force.
            Err(error) => {
                warnings.push(format!("catalogue did not reload: {error}"));
                self.catalog()
            }
        };
        // The provider and the model are two config keys and never a split
        // reference: the resolver takes the pair as it is, and the model keeps
        // every slash it has.
        let spec = match (&next.default_provider, &next.default_model) {
            (Some(provider), Some(model)) => {
                match self.default_spec_for(&next, &catalog, provider, model) {
                    Ok(spec) => Some(spec),
                    // A default provider that the catalogue no longer carries must
                    // not abort the whole reload: the spec already in force stays,
                    // and a new session will report its own error when it tries to
                    // open on the missing provider.
                    Err(error) => {
                        warnings.push(format!("default provider/model not in catalogue: {error}"));
                        self.default_spec()
                    }
                }
            }
            // No default: clear the spec, since a config that only changes
            // unrelated settings must still reload and there is no model to
            // open a session on.
            _ => None,
        };
        let secrets = collect_secrets(&next, &catalog);
        let skills = Arc::new(discover_skills(&next, &self.workspace_root));

        self.replace_config(next.clone());
        *self
            .agent_profiles
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = agent_profiles;
        *self
            .secrets
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = secrets;
        *self
            .default_spec
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = spec;
        *self.lock_skills_write() = skills;

        // Every open session's spec is rebuilt from the new configuration. A
        // spec is resolved once and carries the provider's settings baked into
        // it: the endpoint, the wire, the headers, whether the prompt cache key
        // is sent: so a config that changed one of them changed nothing for a
        // session already open: the record kept the spec built at startup and
        // the next turn used it. Reloading the file and still sending the field
        // a provider rejects is the worst version of this, because the person
        // did what they were told and the request was refused anyway.
        //
        // The reference is not touched: a session stays on the model it was on,
        // and only the settings around it are re-read. A model that no longer
        // resolves is left as it was rather than failing the whole reload, and
        // the turn reports its own error if it comes to that.
        {
            let models: Vec<(SessionKey, String, String)> = self
                .lock_sessions()
                .iter()
                .filter_map(|(key, record)| {
                    let spec = record.spec.as_ref()?;
                    Some((key.clone(), spec.provider.clone(), spec.model.clone()))
                })
                .collect();
            for (key, provider, model) in models {
                if let Ok(spec) = self.default_spec_for(&next, &catalog, &provider, &model)
                    && let Some(record) = self.lock_sessions().get_mut(&key)
                {
                    record.spec = Some(spec);
                }
            }
        }

        // Only the global rules come from the config; the per-session grants the
        // user made during this run are session state and stay.
        for tool in &next.approvals.allow {
            self.approval_rules.allow_global(tool);
        }
        for tool in &next.approvals.deny {
            self.approval_rules.deny_global(tool);
        }
        Ok(ReloadedConfig {
            config: next,
            warnings,
        })
    }
    /// Resolves a provider and a model the way a run does.
    ///
    /// [`ModelSpec::from_catalog`] only knows the catalogue, and a configured
    /// provider is not in it: `river` and every other private endpoint is
    /// declared in `service.json` and nowhere else. A reload that went straight
    /// to the catalogue therefore failed on a default model that a run resolves
    /// without complaint: `run --provider river` worked while `/refresh` said
    /// the provider was unknown, which is the same pair answered two ways.
    ///
    /// So the catalogue is tried first and a configured provider falls back the
    /// same way the command line does. A pair that is neither: a typo, or
    /// a catalogue model that does not exist: is still an error.
    ///
    /// An empty catalogue is not a hard stop: a catalogue plugin feeds the
    /// service an in-memory catalogue at startup (`set_plugin_catalogue`), and
    /// that catalogue may be the only one there is. The message says which
    /// file to edit only when nothing at all was offered, files or plugin.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the pair names neither a catalogue
    /// provider nor a configured one.
    #[allow(dead_code)]
    pub(super) fn default_spec_for_test(
        &self,
        provider: &str,
        model: &str,
    ) -> Result<ModelSpec, ServiceError> {
        let catalog = self.catalog_with_plugins();
        self.default_spec_for(&self.lock_config(), &catalog, provider, model)
    }
    pub(super) fn default_spec_for(
        &self,
        config: &Config,
        catalog: &Catalog,
        provider: &str,
        model: &str,
    ) -> Result<ModelSpec, ServiceError> {
        // The catalogue is the only place a model comes from, so a model it
        // does not carry is not resolved from somewhere else: there is nowhere
        // else. A provider is in `providers.json` or it is unknown, and a model
        // is in `models.json` or it is unknown.
        if provider.trim().is_empty() || model.trim().is_empty() {
            return Err(ServiceError::Unsupported("no model is configured"));
        }
        // The file catalogue first, the plugin catalogue second. A plugin
        // offers pairs that no file carries, and a resolve that skipped it
        // would call those unknown even though the session can use them.
        let spec = match ModelSpec::from_catalog(config, catalog, provider, model, None) {
            Ok(spec) => spec,
            Err(catalog_error) => {
                let merged = self.catalog_with_plugins();
                if Arc::ptr_eq(&merged, &self.catalog()) {
                    return Err(catalog_error);
                }
                ModelSpec::from_catalog(config, &merged, provider, model, None)?
            }
        };
        // The key is in hand now, so it joins the redaction list even when the
        // spec is rebuilt for a session the startup pass never resolved.
        self.remember_secret(&spec.api_key);
        Ok(spec)
    }
    /// The default spec in force.
    ///
    /// The model a new session opens on, and the one `/debug` reports when it
    /// is asked before any message has named a session.
    #[must_use]
    pub fn default_spec(&self) -> Option<ModelSpec> {
        self.default_spec
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    /// The catalogue in use.
    #[must_use]
    pub fn catalog(&self) -> Arc<Catalog> {
        Arc::clone(&self.lock_catalog())
    }
}
