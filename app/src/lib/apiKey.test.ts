import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./ipc", () => import("../test/ipcMock"));

import { loadKeyStatus, needsApiKey } from "./apiKey";
import * as ipc from "../test/ipcMock";

const noKey = { ...ipc.sampleConnection, api_key: "", env_key_available: false };

beforeEach(() => ipc.resetIpc());

describe("up-front API key detection", () => {
  it("reports a resolved key without asking about local models", async () => {
    expect(await loadKeyStatus()).toEqual({ hasKey: true, localModels: [] });
    expect(ipc.installedLocalModels).not.toHaveBeenCalled();
  });

  it("names the models that need no key when none resolves", async () => {
    ipc.getConnection.mockResolvedValue(noKey);
    const status = await loadKeyStatus();
    expect(status).toEqual({ hasKey: false, localModels: ["qwen3-8b-q4-k-m"] });
    expect(needsApiKey(status, "claude-opus-4-8")).toBe(true);
    expect(needsApiKey(status, "qwen3-8b-q4-k-m")).toBe(false);
  });

  it("still asks for a key when the local list can't be read", async () => {
    ipc.getConnection.mockResolvedValue(noKey);
    ipc.installedLocalModels.mockRejectedValue(new Error("models dir unreadable"));
    expect(await loadKeyStatus()).toEqual({ hasKey: false, localModels: [] });
  });

  it("never prompts while the status is unknown", () => {
    expect(needsApiKey(null, "claude-opus-4-8")).toBe(false);
  });
});
