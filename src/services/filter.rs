//! Search, filtering and sorting. Pure functions over the in-memory package
//! list so keystrokes never re-query a package manager.

use crate::models::{Package, Source};
use std::cmp::Ordering;

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub query: String,
    pub show_system: bool,
    pub source: Option<Source>,
    pub category: Option<String>,
}

impl Filter {
    pub fn matches(&self, p: &Package) -> bool {
        if !self.show_system && p.is_system() {
            return false;
        }
        if let Some(s) = self.source {
            if p.source != Some(s) {
                return false;
            }
        }
        if let Some(c) = &self.category {
            if p.category.as_deref() != Some(c.as_str()) {
                return false;
            }
        }
        self.matches_query(p)
    }

    /// Every whitespace separated term must appear in name, publisher,
    /// package identifier, description or source (case-insensitive).
    pub fn matches_query(&self, p: &Package) -> bool {
        let q = self.query.trim();
        if q.is_empty() {
            return true;
        }
        let fields: [Option<&str>; 5] =
            [Some(&p.name), p.publisher.as_deref(), Some(&p.id), p.description.as_deref(), p.source.map(Source::label)];
        let hay: Vec<String> = fields.iter().flatten().map(|f| f.to_lowercase()).collect();
        q.to_lowercase().split_whitespace().all(|term| hay.iter().any(|h| h.contains(term)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Name,
    Publisher,
    InstalledOn,
    Size,
    Version,
    Source,
}

/// Compare two version strings treating digit runs numerically.
pub fn version_cmp(a: &str, b: &str) -> Ordering {
    fn chunks(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.chars() {
            let d = c.is_ascii_digit();
            match out.last_mut() {
                Some((kind, buf)) if *kind == d && (d || c.is_alphabetic()) => buf.push(c),
                _ => out.push((d, c.to_string())),
            }
        }
        out
    }
    let (ca, cb) = (chunks(a), chunks(b));
    for (x, y) in ca.iter().zip(cb.iter()) {
        let o = match (x.0, y.0) {
            (true, true) => {
                let (nx, ny) = (x.1.trim_start_matches('0'), y.1.trim_start_matches('0'));
                nx.len().cmp(&ny.len()).then_with(|| nx.cmp(ny))
            }
            _ => x.1.to_lowercase().cmp(&y.1.to_lowercase()),
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    ca.len().cmp(&cb.len())
}

fn opt_cmp<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    // Missing values sort first when ascending.
    a.cmp(&b)
}

pub fn compare(col: SortColumn, a: &Package, b: &Package) -> Ordering {
    let by_name = || a.name.to_lowercase().cmp(&b.name.to_lowercase());
    match col {
        SortColumn::Name => by_name(),
        SortColumn::Publisher => {
            opt_cmp(a.publisher.as_ref().map(|s| s.to_lowercase()), b.publisher.as_ref().map(|s| s.to_lowercase()))
                .then_with(by_name)
        }
        SortColumn::InstalledOn => opt_cmp(a.installed_on, b.installed_on).then_with(by_name),
        SortColumn::Size => opt_cmp(a.size, b.size).then_with(by_name),
        SortColumn::Version => version_cmp(&a.version, &b.version).then_with(by_name),
        SortColumn::Source => opt_cmp(a.source, b.source).then_with(by_name),
    }
}

/// Indices of matching packages in sorted order.
pub fn select(pkgs: &[Package], filter: &Filter, col: SortColumn, ascending: bool) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..pkgs.len()).filter(|&i| filter.matches(&pkgs[i])).collect();
    idx.sort_by(|&a, &b| {
        let o = compare(col, &pkgs[a], &pkgs[b]);
        if ascending {
            o
        } else {
            o.reverse()
        }
    });
    idx
}

/// Sorted distinct categories present in the list.
pub fn categories(pkgs: &[Package]) -> Vec<String> {
    let mut v: Vec<String> = pkgs.iter().filter_map(|p| p.category.clone()).collect();
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Kind;

    fn p(name: &str, src: Source, kind: Kind) -> Package {
        Package {
            source: Some(src),
            id: name.to_lowercase().replace(' ', "-"),
            name: name.into(),
            kind,
            ..Default::default()
        }
    }

    fn sample() -> Vec<Package> {
        let mut firefox = p("Firefox", Source::Apt, Kind::Application);
        firefox.publisher = Some("Mozilla".into());
        firefox.description = Some("Web browser".into());
        firefox.size = Some(300);
        firefox.version = "128.0".into();
        firefox.category = Some("Internet".into());
        let mut gimp = p("GIMP", Source::Flatpak, Kind::Application);
        gimp.size = Some(100);
        gimp.version = "2.10.9".into();
        gimp.category = Some("Graphics".into());
        let mut libssl = p("libssl3", Source::Apt, Kind::System);
        libssl.size = Some(5);
        libssl.version = "3.0.2".into();
        vec![firefox, gimp, libssl]
    }

    #[test]
    fn hides_system_by_default() {
        let v = sample();
        let f = Filter::default();
        assert_eq!(select(&v, &f, SortColumn::Name, true).len(), 2);
        let f = Filter { show_system: true, ..Default::default() };
        assert_eq!(select(&v, &f, SortColumn::Name, true).len(), 3);
    }

    #[test]
    fn search_fields() {
        let v = sample();
        for (q, n) in
            [("mozilla", 1), ("web", 1), ("gimp", 1), ("flatpak", 1), ("fire fox", 1), ("fire zzz", 0), ("", 2)]
        {
            let f = Filter { query: q.into(), ..Default::default() };
            assert_eq!(select(&v, &f, SortColumn::Name, true).len(), n, "query {q:?}");
        }
        // Package identifier
        let f = Filter { query: "libssl".into(), show_system: true, ..Default::default() };
        assert_eq!(select(&v, &f, SortColumn::Name, true).len(), 1);
    }

    #[test]
    fn source_and_category_filters() {
        let v = sample();
        let f = Filter { source: Some(Source::Flatpak), ..Default::default() };
        assert_eq!(select(&v, &f, SortColumn::Name, true), vec![1]);
        let f = Filter { category: Some("Internet".into()), ..Default::default() };
        assert_eq!(select(&v, &f, SortColumn::Name, true), vec![0]);
    }

    #[test]
    fn sorting() {
        let v = sample();
        let f = Filter { show_system: true, ..Default::default() };
        assert_eq!(select(&v, &f, SortColumn::Name, true), vec![0, 1, 2]);
        assert_eq!(select(&v, &f, SortColumn::Size, true), vec![2, 1, 0]);
        assert_eq!(select(&v, &f, SortColumn::Size, false), vec![0, 1, 2]);
        assert_eq!(select(&v, &f, SortColumn::Source, true)[2], 1);
    }

    #[test]
    fn version_ordering_is_numeric() {
        assert_eq!(version_cmp("2.10.9", "2.9.1"), Ordering::Greater);
        assert_eq!(version_cmp("1.0", "1.0"), Ordering::Equal);
        assert_eq!(version_cmp("1.0", "1.0.1"), Ordering::Less);
        assert_eq!(version_cmp("128.0", "99.1"), Ordering::Greater);
        assert_eq!(version_cmp("1.0a", "1.0b"), Ordering::Less);
    }

    #[test]
    fn category_list() {
        assert_eq!(categories(&sample()), vec!["Graphics", "Internet"]);
    }
}
