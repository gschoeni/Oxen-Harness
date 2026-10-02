//! Release switches, captured once by the host and shared with its frontend.
//!
//! Every flag defaults off and is turned on by setting its environment
//! variable to `1` or `true` before the host starts, so unfinished surfaces
//! can be exercised without a rebuild.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct FeatureFlags {
    /// `OXEN_WORKBENCH_CUSTOMIZATION`: the view picker, View Studio and
    /// installable view packages.
    pub workbench_customization: bool,
    /// `OXEN_WORKFLOWS`: the Oxen workflow node graph and the agent's
    /// `run_workflow` tool, which spends money on generation.
    pub workflows: bool,
    /// `OXEN_ADVANCED_SETTINGS`: desktop controls most users never need —
    /// the custom HTTP tool editor, the code-review step prompts, and the
    /// composer's compression picker with its savings readout.
    pub advanced_settings: bool,
    /// `OXEN_CLI_CANVAS`: the `canvas` tool in the terminal, where it can
    /// only write a file and open a browser.
    pub cli_canvas: bool,
}

impl FeatureFlags {
    pub fn from_env() -> Self {
        let on = |name: &str| enabled(std::env::var(name).as_deref());
        Self {
            workbench_customization: on("OXEN_WORKBENCH_CUSTOMIZATION"),
            workflows: on("OXEN_WORKFLOWS"),
            advanced_settings: on("OXEN_ADVANCED_SETTINGS"),
            cli_canvas: on("OXEN_CLI_CANVAS"),
        }
    }

    pub fn require_workbench_customization(self) -> Result<(), String> {
        if self.workbench_customization {
            Ok(())
        } else {
            Err("Work panel customization is disabled in this release.".into())
        }
    }

    pub fn require_workflows(self) -> Result<(), String> {
        if self.workflows {
            Ok(())
        } else {
            Err("Workflows are disabled in this release.".into())
        }
    }

    pub fn allows_work_view(self, id: &str) -> bool {
        if id == "workflow" {
            return self.workflows;
        }
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
        for id in ["editor", "gallery", "preview", "canvas", "browser"] {
            assert!(flags.allows_work_view(id));
        }
        for id in ["view-studio", "view-manager", "package:demo"] {
            assert!(!flags.allows_work_view(id));
            assert!(FeatureFlags {
                workbench_customization: true,
                ..FeatureFlags::default()
            }
            .allows_work_view(id));
        }
    }

    #[test]
    fn workflows_have_their_own_switch() {
        let off = FeatureFlags::default();
        assert!(off.require_workflows().is_err());
        assert!(!off.allows_work_view("workflow"));
        // Customization alone does not bring the paid workflow view back.
        assert!(!FeatureFlags {
            workbench_customization: true,
            ..off
        }
        .allows_work_view("workflow"));
        let on = FeatureFlags {
            workflows: true,
            ..off
        };
        assert!(on.require_workflows().is_ok());
        assert!(on.allows_work_view("workflow"));
        assert!(!on.allows_work_view("view-studio"));
    }
}
