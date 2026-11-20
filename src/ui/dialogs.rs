//! Confirmation, progress, settings, details and about dialogs.

use super::details::DetailsView;
use crate::models::*;
use crate::services::removal::{Prepared, Risk};
use crate::settings::{Settings, Theme};
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub fn apply_theme(theme: Theme) {
    adw::StyleManager::default().set_color_scheme(match theme {
        Theme::System => adw::ColorScheme::Default,
        Theme::Light => adw::ColorScheme::ForceLight,
        Theme::Dark => adw::ColorScheme::ForceDark,
    });
}

pub fn show_error(parent: &impl IsA<gtk::Widget>, heading: &str, body: &str) {
    let d = adw::AlertDialog::new(Some(heading), Some(body));
    d.add_response("close", "Close");
    d.set_close_response("close");
    d.present(Some(parent));
}

fn small_label(text: &str, css: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_selectable(true);
    if !css.is_empty() {
        l.add_css_class(css);
    }
    l
}

/// Ask the user to confirm a prepared removal. Returns true to proceed.
/// A blocked plan is reported and returns false.
pub async fn confirm_removal(parent: &impl IsA<gtk::Widget>, prepared: &Prepared) -> bool {
    let pkg = &prepared.package;
    if let Some(reason) = &prepared.plan.blocked {
        let d = adw::AlertDialog::new(
            Some(&format!("Cannot uninstall {}", pkg.name)),
            Some(&format!("The package manager will not allow this removal:\n\n{reason}")),
        );
        d.add_response("close", "Close");
        d.set_close_response("close");
        d.choose_future(Some(parent)).await;
        return false;
    }

    let critical = matches!(prepared.risk, Risk::Critical(_));
    let heading = if critical {
        format!("Warning: remove critical component “{}”?", pkg.name)
    } else {
        format!("Uninstall “{}”?", pkg.name)
    };
    let dialog = adw::AlertDialog::new(Some(&heading), None);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let facts = gtk::Grid::new();
    facts.set_column_spacing(12);
    facts.set_row_spacing(2);
    let mut src = pkg.source().label().to_string();
    if let Some(s) = pkg.scope.as_deref().filter(|_| pkg.source == Some(Source::Flatpak)) {
        src = format!("{src} ({s})");
    }
    let rows =
        [("Name", pkg.name.clone()), ("Version", pkg.version.clone()), ("Source", src), ("Package", pkg.id.clone())];
    for (i, (k, v)) in rows.iter().enumerate() {
        let kl = gtk::Label::new(Some(k));
        kl.set_xalign(1.0);
        kl.add_css_class("details-key");
        let vl = small_label(v, "");
        facts.attach(&kl, 0, i as i32, 1, 1);
        facts.attach(&vl, 1, i as i32, 1, 1);
    }
    content.append(&facts);

    match &prepared.risk {
        Risk::Critical(reasons) => {
            let warn = small_label(
                &format!(
                    "Removing this can leave your system unable to boot, log in or update.\n{}",
                    reasons.iter().map(|r| format!("• {r}")).collect::<Vec<_>>().join("\n")
                ),
                "error",
            );
            content.append(&warn);
        }
        Risk::Caution(msg) => content.append(&small_label(msg, "warning")),
        Risk::Normal => {}
    }

    let extra: Vec<_> = prepared.plan.extra_packages().collect();
    if !extra.is_empty() {
        content.append(&small_label(
            &format!("{} other package(s) will also be removed because they depend on this one:", extra.len()),
            "heading",
        ));
        let list = extra
            .iter()
            .map(|p| match &p.version {
                Some(v) => format!("{}  {}", p.name, v),
                None => p.name.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let sw = gtk::ScrolledWindow::new();
        sw.set_min_content_height(60);
        sw.set_max_content_height(160);
        sw.set_propagate_natural_height(true);
        sw.set_child(Some(&small_label(&list, "monospace")));
        sw.add_css_class("card");
        content.append(&sw);
    }
    for w in &prepared.plan.warnings {
        content.append(&small_label(w, "dim-label"));
    }
    if !prepared.plan.simulated && prepared.plan.warnings.is_empty() {
        content.append(&small_label("The package manager could not preview this transaction.", "dim-label"));
    }

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("remove", "Uninstall");
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    if prepared.needs_acknowledgement() {
        let ack = gtk::CheckButton::with_label("I understand the risk and want to remove this anyway");
        dialog.set_response_enabled("remove", false);
        let d2 = dialog.clone();
        ack.connect_toggled(move |c| d2.set_response_enabled("remove", c.is_active()));
        content.append(&ack);
    }
    dialog.set_extra_child(Some(&content));
    dialog.choose_future(Some(parent)).await == "remove"
}

enum Msg {
    Line(String),
    Done(Result<()>),
}

/// Show progress while the removal runs. Resolves when the transaction has
/// finished and the user closed the dialog; returns whether it succeeded.
pub async fn run_removal(parent: &impl IsA<gtk::Widget>, prepared: Prepared) -> bool {
    let dialog = adw::Dialog::builder()
        .title(format!("Uninstalling {}", prepared.package.name))
        .content_width(620)
        .content_height(420)
        .can_close(false)
        .build();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_top(14);
    body.set_margin_bottom(14);
    body.set_margin_start(14);
    body.set_margin_end(14);

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let spinner = gtk::Spinner::new();
    spinner.start();
    let status = gtk::Label::new(Some("Waiting for administrator authentication if required…"));
    status.set_xalign(0.0);
    status.set_hexpand(true);
    status.set_wrap(true);
    top.append(&spinner);
    top.append(&status);
    body.append(&top);

    let buffer = gtk::TextBuffer::new(None);
    let view = gtk::TextView::with_buffer(&buffer);
    view.set_editable(false);
    view.set_monospace(true);
    view.set_cursor_visible(false);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    let sw = gtk::ScrolledWindow::new();
    sw.set_vexpand(true);
    sw.add_css_class("card");
    sw.set_child(Some(&view));
    body.append(&sw);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let close = gtk::Button::with_label("Close");
    close.set_sensitive(false);
    close.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&close);
    body.append(&buttons);

    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::builder().show_end_title_buttons(false).show_start_title_buttons(false).build());
    tv.set_content(Some(&body));
    dialog.set_child(Some(&tv));

    // Cancellation is only offered when the package manager can be
    // interrupted safely (see CommandSpec::cancellable).
    let cancellable = crate::backends::backend_for(prepared.package.source())
        .and_then(|b| b.removal_command(&prepared.package).ok())
        .map(|c| c.cancellable)
        .unwrap_or(false);
    cancel.set_sensitive(cancellable);
    if !cancellable {
        cancel.set_tooltip_text(Some(
            "Package transactions cannot be interrupted safely once started. Dismiss the authentication prompt to abort.",
        ));
    }
    let flag = Arc::new(AtomicBool::new(false));
    {
        let flag = flag.clone();
        let status = status.clone();
        cancel.connect_clicked(move |b| {
            flag.store(true, Ordering::SeqCst);
            b.set_sensitive(false);
            status.set_text("Cancelling…");
        });
    }

    let (tx, rx) = async_channel::unbounded::<Msg>();
    {
        let flag = flag.clone();
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let r = crate::services::removal::execute(&prepared, &flag, &mut |l| {
                let _ = tx2.send_blocking(Msg::Line(l.to_string()));
            });
            let _ = tx.send_blocking(Msg::Done(r));
        });
    }

    let (done_tx, done_rx) = async_channel::bounded::<()>(1);
    {
        let dialog2 = dialog.clone();
        close.connect_clicked(move |_| {
            dialog2.set_can_close(true);
            dialog2.close();
        });
        let done_tx = done_tx.clone();
        dialog.connect_closed(move |_| {
            let _ = done_tx.try_send(());
        });
    }
    dialog.present(Some(parent));

    let mark = buffer.create_mark(None, &buffer.end_iter(), false);
    let ok = Rc::new(RefCell::new(false));
    while let Ok(msg) = rx.recv().await {
        match msg {
            Msg::Line(l) => {
                status.set_text("Removing…");
                let mut end = buffer.end_iter();
                buffer.insert(&mut end, &format!("{l}\n"));
                buffer.move_mark(&mark, &buffer.end_iter());
                view.scroll_mark_onscreen(&mark);
            }
            Msg::Done(r) => {
                spinner.stop();
                spinner.set_visible(false);
                cancel.set_sensitive(false);
                close.set_sensitive(true);
                match r {
                    Ok(()) => {
                        status.set_text("Uninstall completed successfully.");
                        *ok.borrow_mut() = true;
                    }
                    Err(e) => {
                        status.set_text(&format!("Uninstall did not complete: {e}"));
                        status.add_css_class("error");
                    }
                }
                break;
            }
        }
    }
    done_rx.recv().await.ok();
    let result = *ok.borrow();
    result
}

/// Details dialog for double-click; fetches extra metadata off the UI thread.
pub fn show_details(parent: &impl IsA<gtk::Widget>, pkg: &Package, show_icons: bool) {
    let dialog = adw::Dialog::builder().title("Program details").content_width(640).build();
    let view = DetailsView::new(true);
    view.set(Some(pkg), show_icons);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    let sw = gtk::ScrolledWindow::new();
    sw.set_propagate_natural_height(true);
    sw.set_max_content_height(520);
    sw.set_child(Some(&view.root));
    tv.set_content(Some(&sw));
    dialog.set_child(Some(&tv));
    dialog.present(Some(parent));

    let (tx, rx) = async_channel::bounded(1);
    let p = pkg.clone();
    std::thread::spawn(move || {
        let r = crate::backends::backend_for(p.source()).and_then(|b| b.details(&p).ok());
        let _ = tx.send_blocking(r);
    });
    gtk::glib::spawn_future_local(async move {
        if let Ok(Some(full)) = rx.recv().await {
            view.set(Some(&full), show_icons);
        }
    });
}

pub fn show_about(parent: &impl IsA<gtk::Widget>) {
    let d = adw::AboutDialog::builder()
        .application_name("Linux Application Remover")
        .application_icon(crate::APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .comments("Uninstall or change a program. Finds applications installed with APT, DNF, Pacman, Zypper, Flatpak and Snap.")
        .developer_name("Linux Application Remover contributors")
        .license_type(gtk::License::Gpl30)
        .build();
    d.present(Some(parent));
}

pub fn show_settings(parent: &impl IsA<gtk::Widget>, settings: Rc<RefCell<Settings>>, on_change: Rc<dyn Fn()>) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Settings");
    let page = adw::PreferencesPage::new();

    let appearance = adw::PreferencesGroup::builder().title("Appearance").build();
    let theme =
        adw::ComboRow::builder().title("Theme").model(&gtk::StringList::new(&["System", "Light", "Dark"])).build();
    theme.set_selected(match settings.borrow().theme {
        Theme::System => 0,
        Theme::Light => 1,
        Theme::Dark => 2,
    });
    {
        let s = settings.clone();
        let cb = on_change.clone();
        theme.connect_selected_notify(move |r| {
            let t = match r.selected() {
                1 => Theme::Light,
                2 => Theme::Dark,
                _ => Theme::System,
            };
            s.borrow_mut().theme = t;
            apply_theme(t);
            s.borrow().save();
            cb();
        });
    }
    appearance.add(&theme);

    let switch = |title: &str, subtitle: &str, value: bool, apply: fn(&mut Settings, bool)| {
        let row = adw::SwitchRow::builder().title(title).subtitle(subtitle).active(value).build();
        let s = settings.clone();
        let cb = on_change.clone();
        row.connect_active_notify(move |r| {
            apply(&mut s.borrow_mut(), r.is_active());
            s.borrow().save();
            cb();
        });
        row
    };
    let cur = settings.borrow().clone();
    appearance.add(&switch("Show application icons", "Display icons in the program list", cur.show_icons, |s, v| {
        s.show_icons = v
    }));
    appearance.add(&switch(
        "Remember window size",
        "Restore the window size on next launch",
        cur.remember_window_size,
        |s, v| s.remember_window_size = v,
    ));
    page.add(&appearance);

    let behaviour = adw::PreferencesGroup::builder().title("Behavior").build();
    behaviour.add(&switch(
        "Show system programs by default",
        "Include libraries, drivers and other system packages",
        cur.show_system_by_default,
        |s, v| s.show_system_by_default = v,
    ));
    behaviour.add(&switch(
        "Confirm before uninstalling",
        "Critical components and multi-package removals are always confirmed",
        cur.confirm_before_uninstall,
        |s, v| s.confirm_before_uninstall = v,
    ));
    behaviour.add(&switch(
        "Refresh installed programs on launch",
        "Otherwise the last scan is shown until you press Refresh",
        cur.refresh_on_launch,
        |s, v| s.refresh_on_launch = v,
    ));
    page.add(&behaviour);

    dialog.add(&page);
    dialog.present(Some(parent));
}
