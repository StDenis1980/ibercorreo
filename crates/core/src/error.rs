//! The error type every Iber Correo crate returns through `?`.

use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum Error {
    /// A value from a file, server, command or settings file is malformed.
    #[error("invalid {what}: {detail}")]
    Invalid { what: &'static str, detail: String },
    /// A referenced folder, message, event or account does not exist.
    #[error("{what} not found: {id}")]
    NotFound { what: &'static str, id: String },
    /// Input is larger than the limit this build accepts.
    #[error("{what} too large ({size} > {limit})")]
    TooLarge { what: &'static str, size: usize, limit: usize },
    /// Reading or writing local storage failed.
    #[error("storage: {0}")]
    Storage(String),
    /// The operation is planned but not built yet. Never a panic.
    #[error("not supported yet: {0}")]
    Unsupported(&'static str),
}

impl Error {
    pub fn invalid(what: &'static str, detail: impl Into<String>) -> Self {
        Error::Invalid { what, detail: detail.into() }
    }

    pub fn not_found(what: &'static str, id: impl Into<String>) -> Self {
        Error::NotFound { what, id: id.into() }
    }
}
