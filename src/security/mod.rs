//! Authentication, identifier validation and the transaction lock.
//!
//! The GUI never runs as root. Privileged operations are executed through
//! `pkexec` (Polkit) as structured argument vectors, never through a shell.

use crate::models::{CommandSpec, Error, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Validate a package identifier before it is passed to a package manager.
///
/// Accepts the union of characters legal in deb, rpm, pacman, Flatpak and Snap
/// names. Rejects anything that could be parsed as an option or contain shell
/// or path trickery.
pub fn validate_package_id(id: &str) -> Result<()> {
    let bad = || Err(Error::InvalidId(id.to_string()));
    if id.is_empty() || id.len() > 255 || id.starts_with('-') || id.starts_with('.') {
        return bad();
    }
    let ok =
        id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-' | ':' | '@' | '~' | '^'));
    if !ok || id.contains("..") {
        return bad();
    }
    Ok(())
}

/// Validate a Flatpak ref such as `app/org.gnome.Calculator/x86_64/stable`.
pub fn validate_flatpak_ref(r: &str) -> Result<()> {
    let parts: Vec<&str> = r.split('/').collect();
    if parts.len() != 4 || !matches!(parts[0], "app" | "runtime") {
        return Err(Error::InvalidId(r.to_string()));
    }
    for part in &parts[1..] {
        validate_package_id(part).map_err(|_| Error::InvalidId(r.to_string()))?;
    }
    Ok(())
}

/// Process-wide lock preventing two package transactions at once.
static TRANSACTION_ACTIVE: AtomicBool = AtomicBool::new(false);

pub struct TransactionGuard(());

impl TransactionGuard {
    pub fn try_acquire() -> Result<Self> {
        TRANSACTION_ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| TransactionGuard(()))
            .map_err(|_| Error::Busy)
    }

    pub fn is_active() -> bool {
        TRANSACTION_ACTIVE.load(Ordering::SeqCst)
    }
}

impl Drop for TransactionGuard {
    fn drop(&mut self) {
        TRANSACTION_ACTIVE.store(false, Ordering::SeqCst);
    }
}

/// True when running inside a Flatpak sandbox.
pub fn in_flatpak_sandbox() -> bool {
    Path::new("/.flatpak-info").exists()
}

/// Locate an executable on PATH (plus the usual sbin dirs).
pub fn find_program(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    let path = std::env::var("PATH").unwrap_or_default();
    let extra = "/usr/local/sbin:/usr/sbin:/sbin:/usr/local/bin:/usr/bin:/bin";
    path.split(':')
        .chain(extra.split(':'))
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
}

/// Turn a [`CommandSpec`] into the actual argv to execute, adding `pkexec`
/// for privileged commands and `flatpak-spawn --host` inside a sandbox.
/// Returns `(program, args)`.
pub fn build_argv(spec: &CommandSpec) -> Result<(String, Vec<String>)> {
    for (k, v) in &spec.env {
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_uppercase() || c == '_') || v.contains('\0') {
            return Err(Error::Other(format!("invalid environment variable {k}")));
        }
    }
    let sandboxed = in_flatpak_sandbox();
    let mut argv: Vec<String> = Vec::new();
    if sandboxed {
        argv.push("flatpak-spawn".into());
        argv.push("--host".into());
    }
    if spec.privileged {
        argv.push("pkexec".into());
    }
    // `env` carries the environment through pkexec's sanitised environment and
    // across the sandbox boundary.
    if !spec.env.is_empty() && (spec.privileged || sandboxed) {
        argv.push("env".into());
        for (k, v) in &spec.env {
            argv.push(format!("{k}={v}"));
        }
    }
    let program = if sandboxed {
        spec.program.clone()
    } else {
        find_program(&spec.program)
            .map(|p| p.to_string_lossy().into_owned())
            .ok_or(Error::Unavailable("required package manager"))?
    };
    argv.push(program);
    argv.extend(spec.args.iter().cloned());
    let first = argv.remove(0);
    Ok((first, argv))
}

/// Map a process exit status to an authentication problem when pkexec says so.
/// pkexec exits 126 when the user dismissed the dialog or is not authorised
/// and 127 when authentication could not be obtained.
pub fn is_auth_failure(spec: &CommandSpec, code: Option<i32>) -> bool {
    spec.privileged && matches!(code, Some(126) | Some(127))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_ids() {
        for ok in ["firefox", "libstdc++6", "org.mozilla.firefox", "python3.12-venv", "gcc:amd64", "foo_bar", "pkg~1"] {
            assert!(validate_package_id(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-rf", "--purge", "a b", "a;b", "$(x)", "a/b", "a\nb", "..", "a..b", ".hidden", "a`b`"] {
            assert!(validate_package_id(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn validates_flatpak_refs() {
        assert!(validate_flatpak_ref("app/org.gnome.Calculator/x86_64/stable").is_ok());
        assert!(validate_flatpak_ref("app/org.gnome.Calculator/x86_64").is_err());
        assert!(validate_flatpak_ref("app/--x/x86_64/stable").is_err());
        assert!(validate_flatpak_ref("foo/a/b/c").is_err());
    }

    #[test]
    fn transaction_lock_is_exclusive() {
        let g = TransactionGuard::try_acquire().unwrap();
        assert!(matches!(TransactionGuard::try_acquire(), Err(Error::Busy)));
        drop(g);
        assert!(TransactionGuard::try_acquire().is_ok());
    }

    #[test]
    fn auth_codes() {
        let spec = CommandSpec { program: "x".into(), args: vec![], env: vec![], privileged: true, cancellable: false };
        assert!(is_auth_failure(&spec, Some(126)));
        assert!(!is_auth_failure(&spec, Some(1)));
    }
}
