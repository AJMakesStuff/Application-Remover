//! Snap, via the snapd REST API on its local unix socket.

use super::*;
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

pub struct SnapBackend;

const SOCKET: &str = "/run/snapd.socket";

fn snapd_get(path: &str) -> Result<Value> {
    let mut s =
        UnixStream::connect(SOCKET).map_err(|e| Error::Spawn { program: "snapd".into(), message: e.to_string() })?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(20))).ok();
    // HTTP/1.0 so the response is neither chunked nor kept alive.
    write!(s, "GET {path} HTTP/1.0\r\nHost: localhost\r\nAccept: application/json\r\n\r\n")
        .map_err(|e| Error::Other(e.to_string()))?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| Error::Other(e.to_string()))?;
    let text = String::from_utf8_lossy(&buf);
    let body =
        text.split_once("\r\n\r\n").map(|x| x.1).ok_or_else(|| Error::Parse("malformed snapd response".into()))?;
    serde_json::from_str(body).map_err(|e| Error::Parse(format!("snapd JSON: {e}")))
}

impl Backend for SnapBackend {
    fn source(&self) -> Source {
        Source::Snap
    }

    fn is_available(&self) -> bool {
        cmd::exists("snap") && Path::new(SOCKET).exists()
    }

    fn list_installed(&self, desktop: &DesktopIndex) -> Result<Vec<Package>> {
        let v = snapd_get("/v2/snaps")?;
        let mut pkgs = parse_snaps(&v)?;
        // Link desktop entries.
        if let Some(arr) = v.get("result").and_then(Value::as_array) {
            for (p, s) in pkgs.iter_mut().zip(arr.iter()) {
                let entries: Vec<_> = s
                    .get("apps")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a.get("desktop-file").and_then(Value::as_str))
                    .filter_map(|f| desktop.get(Path::new(f)))
                    .collect();
                attach_desktop(p, &entries);
            }
        }
        Ok(pkgs)
    }

    fn details(&self, pkg: &Package) -> Result<Package> {
        Ok(pkg.clone())
    }

    fn is_user_facing(&self, pkg: &Package) -> bool {
        pkg.role == Role::App
    }

    fn plan_removal(&self, pkg: &Package) -> Result<RemovalPlan> {
        security::validate_package_id(&pkg.id)?;
        let mut plan = RemovalPlan::default();
        plan.packages.push(PlannedPackage {
            name: pkg.id.clone(),
            version: Some(pkg.version.clone()),
            role: PlanRole::Target,
        });
        plan.warnings.push(
            "Snap takes an automatic snapshot of the app's data before removing it. snapd refuses to remove a snap that other snaps depend on."
                .into(),
        );
        Ok(plan)
    }

    fn removal_command(&self, pkg: &Package) -> Result<CommandSpec> {
        security::validate_package_id(&pkg.id)?;
        Ok(CommandSpec {
            program: "snap".into(),
            args: vec!["remove".into(), "--".into(), pkg.id.clone()],
            env: vec![],
            privileged: true,
            cancellable: false,
        })
    }
}

/// Parse a snapd `/v2/snaps` response.
pub fn parse_snaps(v: &Value) -> Result<Vec<Package>> {
    let arr = v
        .get("result")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Parse("snapd response has no result list".into()))?;
    let mut out = Vec::new();
    for s in arr {
        let Some(name) = s.get("name").and_then(Value::as_str) else { continue };
        let ty = s.get("type").and_then(Value::as_str).unwrap_or("app");
        let text = |k: &str| s.get(k).and_then(Value::as_str).and_then(none_if_empty);
        let publisher = s
            .get("publisher")
            .and_then(|p| p.get("display-name").or_else(|| p.get("username")))
            .and_then(Value::as_str)
            .and_then(none_if_empty);
        out.push(Package {
            source: Some(Source::Snap),
            id: name.to_string(),
            name: text("title").unwrap_or_else(|| name.to_string()),
            version: text("version").unwrap_or_default(),
            description: text("description").or_else(|| text("summary")),
            publisher,
            size: s.get("installed-size").and_then(Value::as_u64),
            installed_on: text("install-date").as_deref().and_then(parse_iso8601),
            location: text("mount-point").or_else(|| Some(format!("/snap/{name}/current"))),
            role: if ty == "app" { Role::App } else { Role::Component },
            icon: None,
            ..Default::default()
        });
    }
    Ok(out)
}

/// Parse "2024-05-01T12:00:00.123456789Z" (UTC) into a unix timestamp.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    let time = time.trim_end_matches('Z');
    let time = time.split(['+', '.']).next()?;
    let mut t = time.split(':');
    let (hh, mm, ss): (i64, i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    // Days from civil (Howard Hinnant).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"type":"sync","status-code":200,"status":"OK","result":[
      {"id":"abc","title":"Spotify","summary":"Music for everyone","description":"Spotify desktop client","installed-size":330000000,
       "install-date":"2024-05-01T12:00:00.5Z","name":"spotify","publisher":{"id":"x","username":"spotify","display-name":"Spotify"},
       "type":"app","version":"1.2.3","channel":"latest/stable","apps":[{"name":"spotify","desktop-file":"/var/lib/snapd/desktop/applications/spotify_spotify.desktop"}]},
      {"id":"core","name":"core22","type":"base","version":"20240111","installed-size":77000000,"publisher":{"username":"canonical","display-name":"Canonical"}},
      {"id":"snapd","name":"snapd","type":"snapd","version":"2.63","installed-size":1}
    ]}"#;

    #[test]
    fn parses_snapd() {
        let v: Value = serde_json::from_str(SAMPLE).unwrap();
        let p = parse_snaps(&v).unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].name, "Spotify");
        assert_eq!(p[0].id, "spotify");
        assert_eq!(p[0].publisher.as_deref(), Some("Spotify"));
        assert_eq!(p[0].size, Some(330_000_000));
        assert_eq!(p[0].installed_on, Some(1714564800));
        assert_eq!(p[0].role, Role::App);
        assert_eq!(p[1].role, Role::Component);
        assert_eq!(p[2].role, Role::Component);
    }

    #[test]
    fn iso_dates() {
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601("2000-03-01T00:00:01.9Z"), Some(951868801));
        assert_eq!(parse_iso8601("nonsense"), None);
    }

    #[test]
    fn system_snaps_hidden() {
        let v: Value = serde_json::from_str(SAMPLE).unwrap();
        let mut p = parse_snaps(&v).unwrap();
        for x in &mut p {
            crate::services::classify::apply(x);
        }
        assert_eq!(p[0].kind, Kind::Application);
        assert_eq!(p[1].kind, Kind::System);
    }

    #[test]
    fn command() {
        let p = Package { source: Some(Source::Snap), id: "spotify".into(), ..Default::default() };
        let c = SnapBackend.removal_command(&p).unwrap();
        assert_eq!(c.args, vec!["remove", "--", "spotify"]);
        assert!(c.privileged);
    }
}
