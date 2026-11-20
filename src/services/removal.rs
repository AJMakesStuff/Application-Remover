//! Removal workflow: validate -> preview -> assess risk -> execute.

use crate::backends;
use crate::models::*;
use crate::security::{self, TransactionGuard};
use crate::services::classify;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Risk {
    Normal,
    /// A system package (shown because "Show System Programs" is on).
    Caution(String),
    /// Removing this could make the system unusable.
    Critical(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct Prepared {
    pub package: Package,
    pub plan: RemovalPlan,
    pub risk: Risk,
}

impl Prepared {
    pub fn can_proceed(&self) -> bool {
        self.plan.blocked.is_none()
    }

    /// Whether a plain "are you sure" is enough or the user must tick an
    /// extra acknowledgement.
    pub fn needs_acknowledgement(&self) -> bool {
        matches!(self.risk, Risk::Critical(_))
    }
}

pub fn assess(pkg: &Package, plan: &RemovalPlan) -> Risk {
    let mut reasons = Vec::new();
    if let Some(r) = &pkg.critical_reason {
        reasons.push(r.clone());
    }
    for p in plan.packages.iter().filter(|p| p.role != PlanRole::Target) {
        if let Some(r) = classify::critical_name_reason(&p.name) {
            reasons.push(format!("The transaction would also remove: {r}"));
        }
    }
    if !reasons.is_empty() {
        reasons.dedup();
        return Risk::Critical(reasons);
    }
    if pkg.is_system() {
        return Risk::Caution(
            pkg.system_reason
                .clone()
                .map(|r| format!("This is a system component ({r}). Other software may depend on it."))
                .unwrap_or_else(|| "This is a system component. Other software may depend on it.".into()),
        );
    }
    Risk::Normal
}

/// Build the removal preview without touching the system. Blocking.
pub fn prepare(pkg: &Package) -> Result<Prepared> {
    prepare_with(pkg, None)
}

fn prepare_with(pkg: &Package, backend: Option<&dyn backends::Backend>) -> Result<Prepared> {
    security::validate_package_id(&pkg.id)?;
    let owned;
    let backend = match backend {
        Some(b) => b,
        None => {
            owned = backends::backend_for(pkg.source.ok_or_else(|| Error::Other("package has no source".into()))?)
                .ok_or(Error::Unavailable("the package manager for this package"))?;
            owned.as_ref()
        }
    };
    let plan = backend.plan_removal(pkg)?;
    let risk = assess(pkg, &plan);
    Ok(Prepared { package: pkg.clone(), plan, risk })
}

/// Run the removal. Blocking; call from a worker thread. Only one
/// transaction can run at a time.
pub fn execute(prepared: &Prepared, cancel: &Arc<AtomicBool>, sink: &mut dyn FnMut(&str)) -> Result<()> {
    if let Some(reason) = &prepared.plan.blocked {
        return Err(Error::Other(reason.clone()));
    }
    let pkg = &prepared.package;
    security::validate_package_id(&pkg.id)?;
    let _guard = TransactionGuard::try_acquire()?;
    let backend = backends::backend_for(pkg.source.ok_or_else(|| Error::Other("package has no source".into()))?)
        .ok_or(Error::Unavailable("the package manager for this package"))?;
    backend.remove(pkg, cancel, sink)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(id: &str, kind: Kind) -> Package {
        let mut p = Package { source: Some(Source::Apt), id: id.into(), name: id.into(), kind, ..Default::default() };
        p.critical_reason = classify::critical_reason(&p);
        p
    }

    fn plan(names: &[&str]) -> RemovalPlan {
        RemovalPlan {
            packages: names
                .iter()
                .enumerate()
                .map(|(i, n)| PlannedPackage {
                    name: n.to_string(),
                    version: None,
                    role: if i == 0 { PlanRole::Target } else { PlanRole::Dependent },
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn normal_app_is_normal() {
        assert_eq!(assess(&pkg("vlc", Kind::Application), &plan(&["vlc"])), Risk::Normal);
    }

    #[test]
    fn system_package_is_caution() {
        assert!(matches!(assess(&pkg("libfoo2", Kind::System), &plan(&["libfoo2"])), Risk::Caution(_)));
    }

    #[test]
    fn critical_package_is_critical() {
        let r = assess(&pkg("systemd", Kind::System), &plan(&["systemd"]));
        assert!(matches!(r, Risk::Critical(_)));
    }

    #[test]
    fn critical_dependent_escalates_normal_app() {
        let r = assess(&pkg("gnome-tweaks", Kind::Application), &plan(&["gnome-tweaks", "gnome-shell"]));
        assert!(matches!(r, Risk::Critical(_)));
    }

    struct FakeBackend;
    impl backends::Backend for FakeBackend {
        fn source(&self) -> Source {
            Source::Apt
        }
        fn is_available(&self) -> bool {
            true
        }
        fn list_installed(&self, _: &crate::services::desktop::DesktopIndex) -> Result<Vec<Package>> {
            Ok(vec![])
        }
        fn details(&self, p: &Package) -> Result<Package> {
            Ok(p.clone())
        }
        fn plan_removal(&self, p: &Package) -> Result<RemovalPlan> {
            Ok(plan(&[&p.id, "vlc-plugin"]))
        }
        fn removal_command(&self, _: &Package) -> Result<CommandSpec> {
            Err(Error::Other("never executed in tests".into()))
        }
    }

    #[test]
    fn prepare_uses_backend_preview_without_executing() {
        let p = pkg("vlc", Kind::Application);
        let prepared = prepare_with(&p, Some(&FakeBackend)).unwrap();
        assert_eq!(prepared.plan.packages.len(), 2);
        assert!(prepared.can_proceed());
        assert!(!prepared.needs_acknowledgement());
    }

    #[test]
    fn prepare_rejects_bad_ids() {
        let p = pkg("--purge", Kind::Application);
        assert!(matches!(prepare_with(&p, Some(&FakeBackend)), Err(Error::InvalidId(_))));
    }

    #[test]
    fn blocked_plans_never_execute() {
        let mut prepared = prepare_with(&pkg("vlc", Kind::Application), Some(&FakeBackend)).unwrap();
        prepared.plan.blocked = Some("nope".into());
        let cancel = Arc::new(AtomicBool::new(false));
        assert!(execute(&prepared, &cancel, &mut |_| {}).is_err());
    }
}
