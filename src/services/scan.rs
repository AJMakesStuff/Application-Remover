//! Package detection: runs every available backend in parallel, classifies
//! the results and caches them for a fast next launch.

use crate::backends;
use crate::models::*;
use crate::services::classify;
use crate::services::desktop::DesktopIndex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub packages: Vec<Package>,
    /// Backends that were detected on this system.
    pub sources: Vec<Source>,
    /// Backends that failed, with a message.
    pub errors: Vec<(Source, String)>,
}

/// Detect backends and list every installed package. Blocking; run on a
/// worker thread.
pub fn scan() -> ScanResult {
    let desktop = DesktopIndex::scan();
    let backends = backends::available_backends();
    let mut result = ScanResult { sources: backends.iter().map(|b| b.source()).collect(), ..Default::default() };

    let outputs: Vec<(Source, Result<Vec<Package>>)> = std::thread::scope(|s| {
        let handles: Vec<_> = backends
            .iter()
            .map(|b| {
                let desktop = &desktop;
                s.spawn(move || {
                    let r = b.list_installed(desktop).map(|mut pkgs| {
                        for p in &mut pkgs {
                            classify::apply(p);
                            if !b.is_user_facing(p) {
                                p.kind = Kind::System;
                                p.system_reason.get_or_insert_with(|| "not a user-facing application".into());
                            } else {
                                p.kind = Kind::Application;
                                p.system_reason = None;
                            }
                        }
                        pkgs
                    });
                    (b.source(), r)
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });

    for (src, r) in outputs {
        match r {
            Ok(mut p) => result.packages.append(&mut p),
            Err(e) => result.errors.push((src, e.to_string())),
        }
    }
    result
}

fn cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("linux-application-remover").join("packages.json"))
}

pub fn load_cache() -> Option<ScanResult> {
    let text = std::fs::read_to_string(cache_path()?).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_cache(r: &ScanResult) {
    let Some(path) = cache_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(r) {
        // Write atomically so a crash never leaves a truncated cache.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_result_roundtrips_through_json() {
        let r = ScanResult {
            packages: vec![Package {
                source: Some(Source::Apt),
                id: "x".into(),
                name: "X".into(),
                ..Default::default()
            }],
            sources: vec![Source::Apt],
            errors: vec![(Source::Snap, "boom".into())],
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: ScanResult = serde_json::from_str(&s).unwrap();
        assert_eq!(back.packages[0].id, "x");
        assert_eq!(back.errors[0].0, Source::Snap);
    }
}
