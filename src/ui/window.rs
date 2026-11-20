//! Main "Programs and Features" window.

use super::details::{fmt_date, set_icon, DetailsView};
use super::dialogs;
use crate::models::*;
use crate::security::TransactionGuard;
use crate::services::filter::{self, Filter, SortColumn};
use crate::services::{removal, scan};
use crate::settings::Settings;
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;

fn pkg_of(o: &glib::Object) -> Ref<'_, Package> {
    o.downcast_ref::<glib::BoxedAnyObject>().expect("list item is a BoxedAnyObject").borrow::<Package>()
}

fn make_sorter(col: SortColumn) -> gtk::CustomSorter {
    gtk::CustomSorter::new(move |a, b| filter::compare(col, &pkg_of(a), &pkg_of(b)).into())
}

pub struct Ui {
    win: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    banner: adw::Banner,
    store: gio::ListStore,
    filter: Rc<RefCell<Filter>>,
    gfilter: gtk::CustomFilter,
    selection: gtk::SingleSelection,
    column_view: gtk::ColumnView,
    columns: Vec<(String, gtk::ColumnViewColumn)>,
    uninstall_btn: gtk::Button,
    refresh_btn: gtk::Button,
    system_toggle: gtk::ToggleButton,
    source_dd: gtk::DropDown,
    category_dd: gtk::DropDown,
    source_map: RefCell<Vec<Option<Source>>>,
    category_map: RefCell<Vec<Option<String>>>,
    status: gtk::Label,
    spinner: gtk::Spinner,
    details: DetailsView,
    details_frame: gtk::Box,
    settings: Rc<RefCell<Settings>>,
    show_icons: Rc<Cell<bool>>,
    scanning: Cell<bool>,
    busy: Cell<bool>,
    detail_gen: Cell<u64>,
    total_packages: Cell<usize>,
}

impl Ui {
    pub fn build(app: &adw::Application) -> Rc<Ui> {
        let settings = Rc::new(RefCell::new(Settings::load()));
        dialogs::apply_theme(settings.borrow().theme);
        let st = settings.borrow().clone();

        let win = adw::ApplicationWindow::builder()
            .application(app)
            .title(crate::APP_NAME)
            .default_width(if st.remember_window_size { st.window_width } else { 1100 })
            .default_height(if st.remember_window_size { st.window_height } else { 720 })
            .build();
        win.set_size_request(640, 480);

        // --- header bar -----------------------------------------------------
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::WindowTitle::new(crate::APP_NAME, "Uninstall or change a program")));
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search programs"));
        search.set_width_chars(28);
        search.set_tooltip_text(Some("Search by name, publisher, package or description (Ctrl+F)"));
        header.pack_end(&search);
        let menu = gio::Menu::new();
        menu.append(Some("Settings"), Some("win.settings"));
        menu.append(Some("About Linux Application Remover"), Some("win.about"));
        menu.append(Some("Quit"), Some("win.quit"));
        let menu_btn =
            gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).primary(true).build();
        header.pack_end(&menu_btn);

        // --- page heading ---------------------------------------------------
        let heading = gtk::Box::new(gtk::Orientation::Vertical, 2);
        heading.set_margin_start(14);
        heading.set_margin_end(14);
        heading.set_margin_top(10);
        heading.set_margin_bottom(4);
        let h1 = gtk::Label::new(Some("Uninstall or change a program"));
        h1.set_xalign(0.0);
        h1.add_css_class("page-heading");
        let h2 = gtk::Label::new(Some("To uninstall a program, select it from the list and click Uninstall."));
        h2.set_xalign(0.0);
        h2.add_css_class("dim-label");
        heading.append(&h1);
        heading.append(&h2);

        // --- toolbar --------------------------------------------------------
        let strip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        strip.add_css_class("toolbar-strip");
        let uninstall_btn = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().label("Uninstall").icon_name("user-trash-symbolic").build())
            .sensitive(false)
            .build();
        uninstall_btn.set_tooltip_text(Some("Uninstall the selected program (Delete)"));
        uninstall_btn.add_css_class("destructive-action");
        let refresh_btn = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().label("Refresh").icon_name("view-refresh-symbolic").build())
            .build();
        refresh_btn.set_tooltip_text(Some("Rescan installed programs (F5)"));

        let view_menu = gtk::MenuButton::builder().label("View").build();
        let pop_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        pop_box.set_margin_top(8);
        pop_box.set_margin_bottom(8);
        pop_box.set_margin_start(8);
        pop_box.set_margin_end(8);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&pop_box));
        view_menu.set_popover(Some(&popover));

        let sys_toggle = gtk::ToggleButton::with_label("Show System Programs: OFF");
        sys_toggle.set_tooltip_text(Some("Include libraries, drivers, dependencies and other system packages"));

        let source_dd = gtk::DropDown::from_strings(&["All sources"]);
        let category_dd = gtk::DropDown::from_strings(&["All categories"]);

        strip.append(&uninstall_btn);
        strip.append(&refresh_btn);
        strip.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        strip.append(&view_menu);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        strip.append(&spacer);
        strip.append(&source_dd);
        strip.append(&category_dd);
        strip.append(&sys_toggle);

        // --- table ----------------------------------------------------------
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let filter_state =
            Rc::new(RefCell::new(Filter { show_system: st.show_system_by_default, ..Default::default() }));
        let gfilter = {
            let f = filter_state.clone();
            gtk::CustomFilter::new(move |o| f.borrow().matches(&pkg_of(o)))
        };
        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(gfilter.clone()));
        let column_view = gtk::ColumnView::new(None::<gtk::SingleSelection>);
        column_view.set_show_row_separators(true);
        column_view.set_show_column_separators(true);
        column_view.set_reorderable(false);
        column_view.set_enable_rubberband(false);
        column_view.set_single_click_activate(false);
        let sorted = gtk::SortListModel::new(Some(filtered), column_view.sorter());
        let selection = gtk::SingleSelection::new(Some(sorted));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        column_view.set_model(Some(&selection));

        let show_icons = Rc::new(Cell::new(st.show_icons));
        let mut columns = Vec::new();
        let name_col = gtk::ColumnViewColumn::new(Some("Name"), Some(name_factory(show_icons.clone())));
        name_col.set_sorter(Some(&make_sorter(SortColumn::Name)));
        name_col.set_resizable(true);
        name_col.set_expand(true);
        name_col.set_fixed_width(300);
        column_view.append_column(&name_col);
        columns.push(("Name".to_string(), name_col.clone()));

        let mut add = |title: &str, col: SortColumn, width: i32, xalign: f32, f: fn(&Package) -> String| {
            let c = gtk::ColumnViewColumn::new(Some(title), Some(text_factory(f, xalign)));
            c.set_sorter(Some(&make_sorter(col)));
            c.set_resizable(true);
            c.set_fixed_width(width);
            column_view.append_column(&c);
            columns.push((title.to_string(), c));
        };
        add("Publisher", SortColumn::Publisher, 170, 0.0, |p| p.publisher.clone().unwrap_or_default());
        add("Installed On", SortColumn::InstalledOn, 100, 0.0, |p| p.installed_on.map(fmt_date).unwrap_or_default());
        add("Size", SortColumn::Size, 80, 1.0, |p| p.size.map(format_size).unwrap_or_default());
        add("Version", SortColumn::Version, 120, 0.0, |p| p.version.clone());
        add("Source", SortColumn::Source, 80, 0.0, |p| {
            let mut s = p.source.map(|s| s.label().to_string()).unwrap_or_default();
            if p.source == Some(Source::Flatpak) {
                if let Some(scope) = &p.scope {
                    s = format!("{s} ({scope})");
                }
            }
            s
        });
        column_view.sort_by_column(Some(&name_col), gtk::SortType::Ascending);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_vexpand(true);
        scroller.set_hexpand(true);
        scroller.set_child(Some(&column_view));

        // View menu: column visibility + icons + details pane.
        let icons_check = gtk::CheckButton::with_label("Show application icons");
        icons_check.set_active(st.show_icons);
        pop_box.append(&icons_check);
        let details_check = gtk::CheckButton::with_label("Show details pane");
        details_check.set_active(true);
        pop_box.append(&details_check);
        pop_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let cols_label = gtk::Label::new(Some("Columns"));
        cols_label.set_xalign(0.0);
        cols_label.add_css_class("dim-label");
        pop_box.append(&cols_label);
        for (title, col) in columns.iter().skip(1) {
            let cb = gtk::CheckButton::with_label(title);
            cb.set_active(true);
            let col = col.clone();
            cb.connect_toggled(move |c| col.set_visible(c.is_active()));
            pop_box.append(&cb);
        }

        // --- details panel + status bar ------------------------------------
        let details = DetailsView::new(false);
        let details_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        details_frame.add_css_class("details-panel");
        details_frame.append(&details.root);

        let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        status_bar.add_css_class("status-bar");
        let spinner = gtk::Spinner::new();
        let status = gtk::Label::new(Some("Loading installed programs…"));
        status.set_xalign(0.0);
        status_bar.append(&spinner);
        status_bar.append(&status);

        let banner = adw::Banner::new("");
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&banner);
        content.append(&heading);
        content.append(&strip);
        content.append(&scroller);
        content.append(&details_frame);
        content.append(&status_bar);

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&content));
        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&header);
        tv.set_content(Some(&toasts));
        win.set_content(Some(&tv));

        let ui = Rc::new(Ui {
            win,
            toasts,
            banner,
            store,
            filter: filter_state,
            gfilter,
            selection,
            column_view,
            columns,
            uninstall_btn,
            refresh_btn,
            system_toggle: sys_toggle,
            source_dd,
            category_dd,
            source_map: RefCell::new(vec![None]),
            category_map: RefCell::new(vec![None]),
            status,
            spinner,
            details,
            details_frame,
            settings,
            show_icons,
            scanning: Cell::new(false),
            busy: Cell::new(false),
            detail_gen: Cell::new(0),
            total_packages: Cell::new(0),
        });

        // --- wiring ---------------------------------------------------------
        ui.system_toggle.set_active(st.show_system_by_default);
        ui.update_system_toggle_label();
        {
            let u = ui.clone();
            ui.system_toggle.connect_toggled(move |t| {
                u.filter.borrow_mut().show_system = t.is_active();
                u.update_system_toggle_label();
                u.refilter();
            });
        }
        {
            let u = ui.clone();
            search.connect_search_changed(move |s| {
                u.filter.borrow_mut().query = s.text().to_string();
                u.refilter();
            });
        }
        {
            let u = ui.clone();
            ui.source_dd.connect_selected_notify(move |d| {
                let sel = u.source_map.borrow().get(d.selected() as usize).cloned().flatten();
                u.filter.borrow_mut().source = sel;
                u.refilter();
            });
            let u = ui.clone();
            ui.category_dd.connect_selected_notify(move |d| {
                let sel = u.category_map.borrow().get(d.selected() as usize).cloned().flatten();
                u.filter.borrow_mut().category = sel;
                u.refilter();
            });
        }
        {
            let u = ui.clone();
            ui.refresh_btn.connect_clicked(move |_| u.rescan());
            let u = ui.clone();
            ui.uninstall_btn.connect_clicked(move |_| u.start_uninstall());
        }
        {
            let u = ui.clone();
            ui.selection.connect_selected_item_notify(move |_| u.selection_changed());
            let u = ui.clone();
            ui.selection.connect_items_changed(move |_, _, _, _| u.update_status());
            let u = ui.clone();
            ui.column_view.connect_activate(move |_, pos| {
                if let Some(obj) = u.selection.item(pos) {
                    let p = pkg_of(&obj).clone();
                    dialogs::show_details(&u.win, &p, u.show_icons.get());
                }
            });
        }
        {
            let u = ui.clone();
            icons_check.connect_toggled(move |c| {
                u.settings.borrow_mut().show_icons = c.is_active();
                u.settings.borrow().save();
                u.apply_icon_setting();
            });
            let u = ui.clone();
            details_check.connect_toggled(move |c| u.details_frame.set_visible(c.is_active()));
        }

        // Actions and shortcuts.
        let u = ui.clone();
        let settings_action = gio::ActionEntry::builder("settings")
            .activate(move |_: &adw::ApplicationWindow, _, _| {
                let u2 = u.clone();
                let on_change: Rc<dyn Fn()> = Rc::new(move || u2.apply_icon_setting());
                dialogs::show_settings(&u.win, u.settings.clone(), on_change);
            })
            .build();
        let u = ui.clone();
        let about_action = gio::ActionEntry::builder("about")
            .activate(move |_: &adw::ApplicationWindow, _, _| dialogs::show_about(&u.win))
            .build();
        let quit_action =
            gio::ActionEntry::builder("quit").activate(|w: &adw::ApplicationWindow, _, _| w.close()).build();
        ui.win.add_action_entries([settings_action, about_action, quit_action]);
        ui.install_shortcuts(&search);

        {
            let u = ui.clone();
            ui.win.connect_close_request(move |w| {
                if u.busy.get() || TransactionGuard::is_active() {
                    u.toast("A package transaction is running. Wait for it to finish before closing.");
                    return glib::Propagation::Stop;
                }
                let mut s = u.settings.borrow_mut();
                if s.remember_window_size {
                    let (w, h) = w.default_size();
                    s.window_width = w;
                    s.window_height = h;
                }
                s.save();
                glib::Propagation::Proceed
            });
        }

        // Start: show the cache immediately, then refresh in the background.
        let cached = scan::load_cache();
        if let Some(c) = &cached {
            ui.apply_scan(c.clone(), false);
        }
        if cached.is_none() || st.refresh_on_launch {
            ui.rescan();
        }
        ui.selection_changed();
        ui
    }

    /// Developer aid used with LAR_SCREENSHOT: LAR_SYSTEM=1, LAR_SELECT=<n>.
    pub fn debug_prepare(self: &Rc<Self>) {
        if std::env::var_os("LAR_SYSTEM").is_some() {
            self.system_toggle.set_active(true);
        }
        if let Some(n) = std::env::var("LAR_SELECT").ok().and_then(|v| v.parse::<u32>().ok()) {
            self.selection.set_selected(n);
            match std::env::var("LAR_DIALOG").as_deref() {
                Ok("settings") => {
                    let u2 = self.clone();
                    let cb: Rc<dyn Fn()> = Rc::new(move || u2.apply_icon_setting());
                    dialogs::show_settings(&self.win, self.settings.clone(), cb);
                }
                Ok("confirm") => self.start_uninstall(),
                Ok("details") => {
                    if let Some(p) = self.selected() {
                        dialogs::show_details(&self.win, &p, true);
                    }
                }
                _ => {}
            }
        }
    }

    pub fn screenshot(&self, path: &std::path::Path) {
        use gtk::prelude::NativeExt;
        let (w, h) = (self.win.width(), self.win.height());
        let paintable = gtk::WidgetPaintable::new(Some(&self.win));
        let snap = gtk::Snapshot::new();
        paintable.snapshot(&snap, w as f64, h as f64);
        if let (Some(node), Some(renderer)) = (snap.to_node(), self.win.native().and_then(|n| n.renderer())) {
            let tex = renderer.render_texture(&node, None);
            if let Err(e) = tex.save_to_png(path) {
                eprintln!("screenshot failed: {e}");
            }
        } else {
            eprintln!("screenshot: no render node or renderer ({w}x{h})");
        }
    }

    pub fn present(&self) {
        self.win.present();
    }

    fn toast(&self, msg: &str) {
        self.toasts.add_toast(adw::Toast::new(msg));
    }

    fn update_system_toggle_label(&self) {
        let on = self.system_toggle.is_active();
        self.system_toggle.set_label(&format!("Show System Programs: {}", if on { "ON" } else { "OFF" }));
        if on {
            self.system_toggle.add_css_class("warning");
        } else {
            self.system_toggle.remove_css_class("warning");
        }
    }

    fn refilter(&self) {
        self.gfilter.changed(gtk::FilterChange::Different);
        self.update_status();
    }

    fn apply_icon_setting(self: &Rc<Self>) {
        let show = self.settings.borrow().show_icons;
        self.show_icons.set(show);
        // Rebuild the name column's factory so visible rows pick it up.
        if let Some((_, col)) = self.columns.first() {
            col.set_factory(Some(&name_factory(self.show_icons.clone())));
        }
        self.selection_changed();
    }

    fn selected(&self) -> Option<Package> {
        self.selection.selected_item().map(|o| pkg_of(&o).clone())
    }

    fn update_status(&self) {
        let n = self.selection.n_items();
        let mut total: u64 = 0;
        for i in 0..n {
            if let Some(o) = self.selection.item(i) {
                total += pkg_of(&o).size.unwrap_or(0);
            }
        }
        let noun = if self.filter.borrow().show_system { "programs and packages" } else { "programs" };
        let mut text = format!("{n} {noun} installed");
        if total > 0 {
            text.push_str(&format!("  •  Total size: {}", format_size(total)));
        }
        if self.scanning.get() {
            text.push_str("  •  Scanning…");
        }
        self.status.set_text(&text);
    }

    fn selection_changed(self: &Rc<Self>) {
        let sel = self.selected();
        self.uninstall_btn.set_sensitive(sel.is_some() && !self.busy.get());
        self.details.set(sel.as_ref(), self.show_icons.get());
        // Fetch extra metadata (location, long description) off the UI thread.
        if let Some(p) = sel {
            let gen = self.detail_gen.get() + 1;
            self.detail_gen.set(gen);
            let (tx, rx) = async_channel::bounded(1);
            let p2 = p.clone();
            std::thread::spawn(move || {
                let _ = tx.send_blocking(crate::backends::backend_for(p2.source()).and_then(|b| b.details(&p2).ok()));
            });
            let u = self.clone();
            glib::spawn_future_local(async move {
                if let Ok(Some(full)) = rx.recv().await {
                    if u.detail_gen.get() == gen {
                        u.details.set(Some(&full), u.show_icons.get());
                    }
                }
            });
        }
    }

    // ---- scanning -------------------------------------------------------------

    pub fn rescan(self: &Rc<Self>) {
        if self.scanning.get() {
            return;
        }
        self.scanning.set(true);
        self.refresh_btn.set_sensitive(false);
        self.spinner.start();
        self.update_status();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(scan::scan());
        });
        let u = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(result) = rx.recv().await {
                u.apply_scan(result.clone(), true);
                std::thread::spawn(move || scan::save_cache(&result));
            }
            u.scanning.set(false);
            u.refresh_btn.set_sensitive(true);
            u.spinner.stop();
            u.update_status();
        });
    }

    fn apply_scan(self: &Rc<Self>, result: scan::ScanResult, fresh: bool) {
        // Preserve the selection across refreshes.
        let keep = self.selected().map(|p| p.key());
        self.selection.set_selected(gtk::INVALID_LIST_POSITION);
        self.store.remove_all();
        self.total_packages.set(result.packages.len());
        let items: Vec<glib::BoxedAnyObject> = result.packages.iter().cloned().map(glib::BoxedAnyObject::new).collect();
        self.store.extend_from_slice(&items);

        // Source filter choices.
        let cur_source = self.filter.borrow().source;
        let mut labels = vec!["All sources".to_string()];
        let mut map = vec![None];
        let mut sources = result.sources.clone();
        sources.sort();
        for s in sources {
            labels.push(s.label().to_string());
            map.push(Some(s));
        }
        let sel = map.iter().position(|s| *s == cur_source).unwrap_or(0);
        *self.source_map.borrow_mut() = map;
        self.source_dd.set_model(Some(&gtk::StringList::new(&labels.iter().map(String::as_str).collect::<Vec<_>>())));
        self.source_dd.set_selected(sel as u32);

        // Category filter choices.
        let cur_cat = self.filter.borrow().category.clone();
        let mut cats = vec!["All categories".to_string()];
        let mut cmap = vec![None];
        for c in filter::categories(&result.packages) {
            cats.push(c.clone());
            cmap.push(Some(c));
        }
        let csel = cmap.iter().position(|c| *c == cur_cat).unwrap_or(0);
        *self.category_map.borrow_mut() = cmap;
        self.category_dd.set_model(Some(&gtk::StringList::new(&cats.iter().map(String::as_str).collect::<Vec<_>>())));
        self.category_dd.set_selected(csel as u32);

        // Banner for missing / failing package managers.
        if result.sources.is_empty() {
            self.banner.set_title("No supported package manager was found (APT, DNF, Pacman, Zypper, Flatpak, Snap).");
            self.banner.set_revealed(true);
        } else if !result.errors.is_empty() && fresh {
            let msg = result.errors.iter().map(|(s, e)| format!("{}: {e}", s.label())).collect::<Vec<_>>().join("; ");
            self.banner.set_title(&format!("Some package managers could not be read — {msg}"));
            self.banner.set_revealed(true);
        } else {
            self.banner.set_revealed(false);
        }

        self.refilter();
        if let Some(key) = keep {
            for i in 0..self.selection.n_items() {
                if let Some(o) = self.selection.item(i) {
                    if pkg_of(&o).key() == key {
                        self.selection.set_selected(i);
                        break;
                    }
                }
            }
        }
        self.update_status();
    }

    // ---- uninstall ------------------------------------------------------------

    fn start_uninstall(self: &Rc<Self>) {
        let Some(pkg) = self.selected() else {
            self.toast("Select a program to uninstall first.");
            return;
        };
        if self.busy.get() || TransactionGuard::is_active() {
            self.toast("Another package transaction is already running.");
            return;
        }
        self.busy.set(true);
        self.uninstall_btn.set_sensitive(false);
        self.status.set_text(&format!("Calculating what removing “{}” would change…", pkg.name));
        self.spinner.start();

        let (tx, rx) = async_channel::bounded(1);
        let p = pkg.clone();
        std::thread::spawn(move || {
            let _ = tx.send_blocking(removal::prepare(&p));
        });
        let u = self.clone();
        glib::spawn_future_local(async move {
            let outcome = u.uninstall_flow(rx.recv().await.ok()).await;
            u.busy.set(false);
            u.spinner.stop();
            u.update_status();
            u.selection_changed();
            if outcome {
                u.rescan();
            }
        });
    }

    /// Returns true when a removal ran successfully.
    async fn uninstall_flow(self: &Rc<Self>, prepared: Option<Result<removal::Prepared>>) -> bool {
        let prepared = match prepared {
            Some(Ok(p)) => p,
            Some(Err(e)) => {
                dialogs::show_error(&self.win, "Cannot prepare uninstall", &e.to_string());
                return false;
            }
            None => return false,
        };
        // The setting may skip the plain dialog, but never for risky or
        // multi-package removals, and never when blocked.
        let simple = prepared.risk == removal::Risk::Normal
            && prepared.plan.extra_packages().next().is_none()
            && prepared.can_proceed();
        let skip_dialog = simple && !self.settings.borrow().confirm_before_uninstall;
        if !skip_dialog && !dialogs::confirm_removal(&self.win, &prepared).await {
            return false;
        }
        let name = prepared.package.name.clone();
        let ok = dialogs::run_removal(&self.win, prepared).await;
        self.toast(&if ok {
            format!("“{name}” was uninstalled.")
        } else {
            format!("“{name}” was not uninstalled.")
        });
        ok
    }

    // ---- shortcuts ------------------------------------------------------------

    fn install_shortcuts(self: &Rc<Self>, search: &gtk::SearchEntry) {
        let ctl = gtk::ShortcutController::new();
        ctl.set_scope(gtk::ShortcutScope::Managed);
        let add = |accel: &str, f: Box<dyn Fn() + 'static>| {
            if let Some(trigger) = gtk::ShortcutTrigger::parse_string(accel) {
                let action = gtk::CallbackAction::new(move |_, _| {
                    f();
                    glib::Propagation::Stop
                });
                ctl.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(action)));
            }
        };
        let u = self.clone();
        add("F5|<Control>r", Box::new(move || u.rescan()));
        let s = search.clone();
        add(
            "<Control>f",
            Box::new(move || {
                s.grab_focus();
            }),
        );
        let u = self.clone();
        add(
            "<Control>comma",
            Box::new(move || {
                let u2 = u.clone();
                let cb: Rc<dyn Fn()> = Rc::new(move || u2.apply_icon_setting());
                dialogs::show_settings(&u.win, u.settings.clone(), cb);
            }),
        );
        self.win.add_controller(ctl);

        // Delete uninstalls only when the table (not the search box) has focus.
        let key = gtk::EventControllerKey::new();
        let u = self.clone();
        key.connect_key_pressed(move |_, k, _, _| {
            if k == gdk::Key::Delete {
                u.start_uninstall();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.column_view.add_controller(key);
    }
}

// ---- cell factories -------------------------------------------------------------

fn text_factory(f: fn(&Package) -> String, xalign: f32) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let l = gtk::Label::new(None);
        l.set_xalign(xalign);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        item.set_child(Some(&l));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let (Some(label), Some(obj)) = (item.child().and_downcast::<gtk::Label>(), item.item()) else { return };
        let p = pkg_of(&obj);
        let text = f(&p);
        label.set_text(&text);
        label.set_tooltip_text(if text.is_empty() { None } else { Some(&text) });
        if p.is_system() {
            label.add_css_class("dim-label");
        } else {
            label.remove_css_class("dim-label");
        }
    });
    factory
}

fn name_factory(show_icons: Rc<Cell<bool>>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let img = gtk::Image::new();
        img.set_pixel_size(16);
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_hexpand(true);
        let badge = gtk::Label::new(Some("System"));
        badge.add_css_class("system-badge");
        row.append(&img);
        row.append(&label);
        row.append(&badge);
        item.set_child(Some(&row));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let (Some(row), Some(obj)) = (item.child().and_downcast::<gtk::Box>(), item.item()) else { return };
        let (Some(img), Some(label), Some(badge)) = (
            row.first_child().and_downcast::<gtk::Image>(),
            img_next(&row, 1).and_downcast::<gtk::Label>(),
            row.last_child().and_downcast::<gtk::Label>(),
        ) else {
            return;
        };
        let p = pkg_of(&obj);
        label.set_text(&p.name);
        label.set_tooltip_text(Some(&p.name));
        img.set_visible(show_icons.get());
        if show_icons.get() {
            set_icon(&img, p.icon.as_deref());
        }
        badge.set_visible(p.is_system());
        if p.is_system() {
            label.add_css_class("dim-label");
        } else {
            label.remove_css_class("dim-label");
        }
    });
    factory
}

fn img_next(row: &gtk::Box, n: usize) -> Option<gtk::Widget> {
    let mut w = row.first_child();
    for _ in 0..n {
        w = w.and_then(|x| x.next_sibling());
    }
    w
}
