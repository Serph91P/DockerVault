//! DockerVault application services, runtime ownership and protocol adapters.
pub mod config;
pub mod ownership;
#[cfg(target_os = "linux")]
pub mod process;
