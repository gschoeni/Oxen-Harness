import { beforeEach, expect, it, vi } from "vitest";
vi.mock("./ipc", () => ({ loadFeatureFlags: vi.fn() }));
import { loadFeatureFlags } from "./ipc";
import {
  advancedSettingsEnabled,
  initFeatureFlags,
  workbenchCustomizationEnabled,
  workflowsEnabled,
  type FeatureFlags,
} from "./features";

const flags = (on: Partial<FeatureFlags>): FeatureFlags => ({
  workbench_customization: false,
  workflows: false,
  advanced_settings: false,
  ...on,
});

beforeEach(() => {
  vi.mocked(loadFeatureFlags).mockReset();
});

it("defaults customization off and uses the host's startup flags", async () => {
  expect(workbenchCustomizationEnabled()).toBe(false);
  vi.mocked(loadFeatureFlags).mockResolvedValue(flags({ workbench_customization: true }));
  await initFeatureFlags();
  expect(workbenchCustomizationEnabled()).toBe(true);
  vi.mocked(loadFeatureFlags).mockResolvedValue(flags({}));
  await initFeatureFlags();
  expect(workbenchCustomizationEnabled()).toBe(false);
});

it("leaves customization disabled if the host flags cannot be loaded", async () => {
  vi.mocked(loadFeatureFlags).mockResolvedValue(flags({ workbench_customization: true }));
  await initFeatureFlags();
  vi.mocked(loadFeatureFlags).mockRejectedValue(new Error("host unavailable"));
  await expect(initFeatureFlags()).rejects.toThrow("host unavailable");
  expect(workbenchCustomizationEnabled()).toBe(false);
});

it("switches workflows and advanced settings independently, both off by default", async () => {
  vi.mocked(loadFeatureFlags).mockResolvedValue(flags({}));
  await initFeatureFlags();
  expect(workflowsEnabled()).toBe(false);
  expect(advancedSettingsEnabled()).toBe(false);
  vi.mocked(loadFeatureFlags).mockResolvedValue(flags({ workflows: true }));
  await initFeatureFlags();
  expect(workflowsEnabled()).toBe(true);
  expect(advancedSettingsEnabled()).toBe(false);
  vi.mocked(loadFeatureFlags).mockResolvedValue(flags({ advanced_settings: true }));
  await initFeatureFlags();
  expect(workflowsEnabled()).toBe(false);
  expect(advancedSettingsEnabled()).toBe(true);
});
