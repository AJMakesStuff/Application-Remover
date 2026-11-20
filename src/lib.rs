//! Linux Application Remover: a "Programs and Features" style uninstaller.

pub mod backends;
pub mod models;
pub mod security;
pub mod services;
pub mod settings;

#[cfg(feature = "gui")]
pub mod ui;

pub const APP_ID: &str = "io.github.linux_application_remover";
pub const APP_NAME: &str = "Programs and Features";
