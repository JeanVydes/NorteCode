//! Shared telemetry snapshots used by the run loop and views.

use std::sync::Arc;

/// A telemetry report shared without copying its records.
pub(crate) type SharedTelemetry = Arc<norte_code_wire::TelemetryReport>;
