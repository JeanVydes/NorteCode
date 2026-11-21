//! Outcomes of the key prompt and the intent that armed it.

/// What to do after the key is saved.
#[derive(Clone, Debug)]
pub(crate) enum KeyIntent {
    /// Save the key, then switch the session to this model.
    SwitchModel {
        /// The provider the model belongs to.
        provider: String,
        /// The model id.
        model: String,
    },
    /// Save the key, then expand the provider's models.
    Expand {
        /// The provider to expand.
        provider: String,
    },
}

/// What the prompt reports to the host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum KeyPromptAction {
    /// Store this key, then run the armed intent.
    Submit {
        /// The provider the key belongs to.
        provider: String,
        /// The secret value.
        key: String,
    },
    /// The person dismissed the prompt.
    Cancel,
}
