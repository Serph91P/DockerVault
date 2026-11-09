//! DockerVault application services, runtime ownership and protocol adapters.
pub mod auth;
pub mod config;
pub mod db;
pub mod ownership;
#[cfg(target_os = "linux")]
pub mod process;
