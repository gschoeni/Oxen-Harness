import { loadFeatureFlags } from "./ipc";

export interface FeatureFlags {
  workbench_customization: boolean;
}

let flags: FeatureFlags = { workbench_customization: false };

/** Load once before the store and view registry; the host owns release flags. */
export async function initFeatureFlags() {
  flags = { workbench_customization: false };
  flags = await loadFeatureFlags();
}

export const workbenchCustomizationEnabled = () => flags.workbench_customization;
