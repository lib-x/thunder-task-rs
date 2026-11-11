//! Errors returned by authentication, transport, and API operations.

use thiserror::Error;

/// Library error type.
#[derive(Debug, Error)]
pub enum Error {
    /// HTTP transport or non-2xx response.
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// The server answered with a structured error.
    #[error("API error {code:?}: {message}")]
    Api {
        /// Server error code, including numeric codes converted to strings.
        code: Option<String>,
        /// Human-readable server error description.
        message: String,
    },

    /// Device-flow login is still waiting for user approval.
    #[error("login pending user authorization")]
    LoginPending,

    /// The device-flow login was rejected or expired; restart it.
    #[error("login flow expired or denied")]
    LoginExpired,

    /// Credentials are missing or the token expired and refresh failed.
    #[error("not authenticated (token missing or refresh failed)")]
    NotAuthenticated,

    /// The requested device was not found among authorized devices.
    #[error("device not found: {0}")]
    DeviceNotFound(String),

    /// The response body could not be parsed as the expected shape.
    #[error("unexpected response shape: {0}")]
    UnexpectedResponse(String),

    /// A required configuration value was not provided.
    #[error("missing configuration: {0}")]
    MissingConfig(&'static str),
}

/// Convenient result alias.
pub type Result<T> = std::result::Result<T, Error>;
