//! # thunder-task-rs
//!
//! Rust client for the Xunlei (迅雷) Pan download-task API, the same cloud
//! API that the official mobile and NAS clients use.
//!
//! What this library does:
//! - device-flow login ([`auth`]): obtain a login QR code / verification URL,
//!   wait for the user to approve it in the Xunlei app, then hold and refresh
//!   tokens
//! - devices ([`device`]): list the devices (NAS boxes, PCs) authorized for
//!   your account
//! - tasks ([`task`]): create download tasks targeted at a specific device,
//!   list running/finished tasks, pause/resume/delete
//!
//! What this library does **not** do: it does not bypass any server-side
//! account limits, does not forge identities, and does not reimplement any
//! proprietary signing scheme. All calls use the account you log in with and
//! the server is free to enforce whatever quota applies to it.

pub mod auth;
pub mod client;
pub mod device;
pub mod error;
pub mod local;
pub mod task;
pub mod types;

pub use client::{Client, ClientConfig};
pub use error::{Error, Result};
pub use local::LocalNas;

/// Default authentication host (token + device-code endpoints).
pub const AUTH_HOST: &str = "https://xluser-ssl.xunlei.com";
/// Default cloud API host for drive/task operations.
pub const DRIVE_HOST: &str = "https://api-gateway-pan.xunlei.com";
/// Drive API path prefix. Verified live: `https://api-gateway-pan.xunlei.com/
/// drive/v1/tasks` answers 401 (auth required), while the `/xlppc/...` variant
/// returns 404.
pub const DRIVE_PREFIX: &str = "/drive/v1";

/// Account space constant: the personal cloud drive.
pub const SPACE_USER: &str = "";
/// Task type: a user-initiated download submitted to a device runner.
pub const TYPE_USER_DOWNLOAD: &str = "user#download";
