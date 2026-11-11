pub mod analysis;
pub mod automation;
pub mod config;
pub mod ffi;
pub mod investigation;
pub mod model;
pub mod preferences;
pub mod query;
pub mod recording;
pub mod sharing;
pub mod telemetry;
pub mod workspace;

#[cfg(windows)]
pub mod ui;
#[cfg(windows)]
pub mod windows_workbench;

#[cfg(windows)]
mod windows_scroll;
