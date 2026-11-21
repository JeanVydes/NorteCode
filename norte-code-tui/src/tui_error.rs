//! The error the terminal loop reports to its caller.

use std::error::Error;
use std::fmt;
use std::io;

/// Why the terminal interface stopped before it could run.
#[derive(Debug)]
pub enum TuiError {
    /// The terminal could not be prepared or restored.
    Io(io::Error),
}

impl fmt::Display for TuiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "terminal error: {error}"),
        }
    }
}

impl Error for TuiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
        }
    }
}

impl From<io::Error> for TuiError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
