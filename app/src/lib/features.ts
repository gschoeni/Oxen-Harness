import { loadFeatureFlags } from "./ipc";

/** Mirrors `harness_config::features::FeatureFlags`; every flag defaults off. */
export interface FeatureFlags {
  workbench_customization: boolean;
  workflows: boolean;
  advanced_settings: boolean;
}

const OFF: FeatureFlags = {
  workbench_customization: false,
  workflows: false,
  advanced_settings: false,
};

let flags: FeatureFlags = OFF;

/** Load once before the store and view registry; the host owns release flags. */
export async function initFeatureFlags() {
  flags = OFF;
  flags = { ...OFF, ...(await loadFeatureFlags()) };
}

export const workbenchCustomizationEnabled = () => flags.workbench_customization;
export const workflowsEnabled = () => flags.workflows;
export const advancedSettingsEnabled = () => flags.advanced_settings;
