//! DockerVault application services, runtime ownership and protocol adapters.
pub mod auth;
pub mod config;
pub mod db;
#[cfg(target_os = "linux")]
pub mod docker;
#[cfg(target_os = "linux")]
pub mod hooks;
pub mod http;
pub mod journal;
pub mod ownership;
#[cfg(target_os = "linux")]
pub mod process;
pub mod restore;
#[cfg(target_os = "linux")]
pub mod storage;
#[cfg(target_os = "linux")]
pub mod workload_recovery;
