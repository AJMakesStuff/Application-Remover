//! GTK4 / libadwaita interface.

mod details;
mod dialogs;
mod window;

use adw::prelude::*;
use gtk::{gdk, gio};

const CSS: &str = "
columnview row cell { padding: 1px 6px; min-height: 22px; }
columnview > header button { padding: 3px 8px; }
.page-heading { font-size: 1.35em; color: @accent_color; }
.system-badge { font-size: 0.75em; padding: 0 6px; border-radius: 4px; background: alpha(@warning_color, 0.28); }
.details-key { opacity: 0.65; }
.details-panel { border-top: 1px solid @borders; background: alpha(@window_fg_color, 0.03); }
.status-bar { border-top: 1px solid @borders; padding: 3px 10px; }
.toolbar-strip { padding: 4px 10px; }
";

pub fn run() -> gtk::glib::ExitCode {
    let app = adw::Application::builder().application_id(crate::APP_ID).flags(gio::ApplicationFlags::default()).build();
    app.connect_startup(|_| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
    });
    app.connect_activate(|app| {
        if let Some(w) = app.active_window() {
            w.present();
            return;
        }
        let ui = window::Ui::build(app);
        ui.present();
        // Developer aid: LAR_SCREENSHOT=/path/out.png saves the window and quits.
        if let Some(path) = std::env::var_os("LAR_SCREENSHOT") {
            let app = app.clone();
            gtk::glib::timeout_add_local_once(std::time::Duration::from_secs(5), move || {
                ui.debug_prepare();
                let (ui2, app2) = (ui.clone(), app.clone());
                gtk::glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
                    ui2.screenshot(std::path::Path::new(&path));
                    app2.quit();
                });
            });
        }
    });
    app.run()
}
