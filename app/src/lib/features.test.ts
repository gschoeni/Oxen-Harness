import { beforeEach, expect, it, vi } from "vitest";
vi.mock("./ipc", () => ({ loadFeatureFlags: vi.fn() }));
import { loadFeatureFlags } from "./ipc";
import { initFeatureFlags, workbenchCustomizationEnabled } from "./features";

beforeEach(() => { vi.mocked(loadFeatureFlags).mockReset(); });

it("defaults customization off and uses the host's startup flags", async () => {
  expect(workbenchCustomizationEnabled()).toBe(false);
  vi.mocked(loadFeatureFlags).mockResolvedValue({ workbench_customization: true });
  await initFeatureFlags();
  expect(workbenchCustomizationEnabled()).toBe(true);
  vi.mocked(loadFeatureFlags).mockResolvedValue({ workbench_customization: false });
  await initFeatureFlags();
  expect(workbenchCustomizationEnabled()).toBe(false);
});

it("leaves customization disabled if the host flags cannot be loaded", async () => {
  vi.mocked(loadFeatureFlags).mockResolvedValue({ workbench_customization: true });
  await initFeatureFlags();
  vi.mocked(loadFeatureFlags).mockRejectedValue(new Error("host unavailable"));
  await expect(initFeatureFlags()).rejects.toThrow("host unavailable");
  expect(workbenchCustomizationEnabled()).toBe(false);
});
