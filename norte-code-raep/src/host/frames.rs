//! One domain of the RAEP host, as `impl NorteCodeRaepHost` blocks.

use raep_types::Envelope;
use serde_json::Value;

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::capabilities::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::payloads::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::stores::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

use super::NorteCodeRaepHost;

impl NorteCodeRaepHost {
    /// Rejects a standard operation that this host has not implemented.
    pub(crate) fn not_supported(
        _frame: &Envelope<Value>,
        what: &str,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        Err(HostError::InvalidFrame(format!(
            "{what} is not supported by this host"
        )))
    }
    pub(crate) fn approval_response(
        frame: &Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        Self::not_supported(frame, "TOOL_APPROVAL_RESPONSE")
    }
    pub(crate) fn input_stream(
        frame: &Envelope<Value>,
        what: &str,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        Self::not_supported(frame, &format!("INPUT_STREAM_{what}"))
    }
    pub(crate) fn client_tool_result(
        frame: &Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        Self::not_supported(frame, "CLIENT_TOOL_RESULT")
    }
    pub(crate) fn presence_update(
        frame: &Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        Self::not_supported(frame, "PRESENCE_UPDATE")
    }
    pub(crate) fn handoff(frame: &Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        Self::not_supported(frame, "HANDOFF")
    }
    pub(crate) fn auth_refresh(frame: &Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        Self::not_supported(frame, "AUTH_REFRESH")
    }
    /// Bundle import is implemented over HTTP, not as a wire-frame request.
    pub(crate) fn import() -> Result<Vec<Envelope<Value>>, HostError> {
        Err(HostError::InvalidFrame(
            "SESSION_IMPORT is an HTTP workflow; use POST /raep/v1/imports".to_owned(),
        ))
    }
}
