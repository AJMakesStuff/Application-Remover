//! The details view used by the bottom panel and the details dialog.

use crate::models::*;
use gtk::prelude::*;

#[derive(Clone)]
pub struct DetailsView {
    pub root: gtk::Box,
    icon: gtk::Image,
    title: gtk::Label,
    subtitle: gtk::Label,
    values: Vec<(&'static str, gtk::Label)>,
}

const FIELDS: [&str; 9] =
    ["Publisher", "Version", "Size", "Installed on", "Source", "Package", "Architecture", "Location", "Type"];

pub fn fmt_date(ts: i64) -> String {
    gtk::glib::DateTime::from_unix_local(ts)
        .ok()
        .and_then(|d| d.format("%x").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

pub fn set_icon(img: &gtk::Image, icon: Option<&str>) {
    let theme = gtk::gdk::Display::default().map(|d| gtk::IconTheme::for_display(&d));
    match icon {
        Some(i) if i.starts_with('/') && std::path::Path::new(i).exists() => img.set_from_file(Some(i)),
        Some(i) if theme.as_ref().is_some_and(|t| t.has_icon(i)) => img.set_icon_name(Some(i)),
        _ => img.set_icon_name(Some("application-x-executable")),
    }
}

impl DetailsView {
    pub fn new(large: bool) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        root.set_margin_top(8);
        root.set_margin_bottom(8);
        root.set_margin_start(12);
        root.set_margin_end(12);

        let icon = gtk::Image::from_icon_name("application-x-executable");
        icon.set_pixel_size(if large { 64 } else { 48 });
        icon.set_valign(gtk::Align::Start);
        root.append(&icon);

        let right = gtk::Box::new(gtk::Orientation::Vertical, 4);
        right.set_hexpand(true);
        let title = gtk::Label::new(None);
        title.set_xalign(0.0);
        title.add_css_class("title-4");
        title.set_selectable(true);
        title.set_wrap(true);
        let subtitle = gtk::Label::new(None);
        subtitle.set_xalign(0.0);
        subtitle.set_wrap(true);
        subtitle.set_selectable(true);
        subtitle.add_css_class("dim-label");
        right.append(&title);
        right.append(&subtitle);

        let grid = gtk::Grid::new();
        grid.set_column_spacing(10);
        grid.set_row_spacing(2);
        grid.set_margin_top(4);
        let mut values = Vec::new();
        // Two columns of key/value pairs to keep the panel compact.
        for (i, key) in FIELDS.iter().enumerate() {
            let (row, col) = ((i / 3) as i32, (i % 3) as i32 * 2);
            let k = gtk::Label::new(Some(&format!("{key}:")));
            k.set_xalign(1.0);
            k.set_valign(gtk::Align::Start);
            k.add_css_class("details-key");
            let v = gtk::Label::new(None);
            v.set_xalign(0.0);
            v.set_hexpand(true);
            v.set_selectable(true);
            v.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            v.set_max_width_chars(if large { 60 } else { 34 });
            if large {
                v.set_wrap(true);
                v.set_ellipsize(gtk::pango::EllipsizeMode::None);
            }
            grid.attach(&k, col, row, 1, 1);
            grid.attach(&v, col + 1, row, 1, 1);
            values.push((*key, v));
        }
        right.append(&grid);
        root.append(&right);
        let me = Self { root, icon, title, subtitle, values };
        me.set(None, true);
        me
    }

    fn put(&self, key: &str, value: Option<String>) {
        if let Some((_, l)) = self.values.iter().find(|(k, _)| *k == key) {
            match value {
                Some(v) if !v.is_empty() => {
                    l.set_text(&v);
                    l.set_tooltip_text(Some(&v));
                    l.remove_css_class("dim-label");
                }
                _ => {
                    l.set_text("Not available");
                    l.set_tooltip_text(None);
                    l.add_css_class("dim-label");
                }
            }
        }
    }

    pub fn set(&self, pkg: Option<&Package>, show_icon: bool) {
        let Some(p) = pkg else {
            self.title.set_text("No program selected");
            self.subtitle.set_text("Select a program to see its details.");
            self.icon.set_icon_name(Some("application-x-executable"));
            for (_, l) in &self.values {
                l.set_text("");
            }
            return;
        };
        self.icon.set_visible(show_icon);
        super::details::set_icon(&self.icon, p.icon.as_deref());
        self.title.set_text(&p.name);
        self.subtitle.set_text(p.description.as_deref().map(first_paragraph).unwrap_or("No description available."));
        self.put("Publisher", p.publisher.clone());
        self.put("Version", Some(p.version.clone()));
        self.put("Size", p.size.map(format_size));
        self.put("Installed on", p.installed_on.map(fmt_date));
        let mut src = p.source.map(|s| s.label().to_string()).unwrap_or_default();
        if let Some(scope) = p.scope.as_deref().filter(|_| p.source == Some(Source::Flatpak)) {
            src = format!("{src} ({scope})");
        }
        self.put("Source", Some(src));
        self.put("Package", Some(p.id.clone()));
        self.put("Architecture", p.arch.clone());
        self.put("Location", p.location.clone());
        let kind = if p.is_system() {
            format!("System package{}", p.system_reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default())
        } else {
            "Application".to_string()
        };
        self.put("Type", Some(kind));
    }
}

fn first_paragraph(s: &str) -> &str {
    s.trim().split("\n\n").next().unwrap_or(s).trim()
}
