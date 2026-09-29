//! Release switches, captured once by the host and shared with its frontend.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct FeatureFlags {
    pub workbench_customization: bool,
}

impl FeatureFlags {
    pub fn from_env() -> Self {
        Self {
            workbench_customization: enabled(
                std::env::var("OXEN_WORKBENCH_CUSTOMIZATION").as_deref(),
            ),
        }
    }

    pub fn require_workbench_customization(self) -> Result<(), String> {
        if self.workbench_customization {
            Ok(())
        } else {
            Err("Work panel customization is disabled in this release.".into())
        }
    }

    pub fn allows_work_view(self, id: &str) -> bool {
        self.workbench_customization
            || !(matches!(id, "view-studio" | "view-manager") || id.starts_with("package:"))
    }
}

fn enabled(value: Result<&str, &std::env::VarError>) -> bool {
    matches!(value, Ok("1" | "true"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn customization_requires_explicit_opt_in() {
        for value in [None, Some(""), Some("0"), Some("false"), Some("yes")] {
            assert!(!enabled(value.ok_or(&std::env::VarError::NotPresent)));
        }
        for value in ["1", "true"] {
            assert!(enabled(Ok(value)));
        }
        let flags = FeatureFlags::default();
        assert!(flags.require_workbench_customization().is_err());
        for id in [
            "editor", "gallery", "preview", "canvas", "browser", "workflow",
        ] {
            assert!(flags.allows_work_view(id));
        }
        for id in ["view-studio", "view-manager", "package:demo"] {
            assert!(!flags.allows_work_view(id));
            assert!(FeatureFlags {
                workbench_customization: true
            }
            .allows_work_view(id));
        }
    }
}
