// Knowing, before a turn is sent, whether it would fail for want of an Oxen
// API key — so a new user is asked for one up front instead of after a 401.
//
// The host has no single "can this chat reach its model?" command, so the
// answer is assembled from two that exist: `get_connection` says whether any
// key resolves, and `installed_local_models` says which model ids run
// on-device and so need none.

import { getConnection, installedLocalModels } from "./ipc";

export interface KeyStatus {
  /** Whether a key resolves for the current endpoint (saved, env, or CLI login). */
  hasKey: boolean;
  /** Ids of on-device models, which a chat can use without a key. */
  localModels: string[];
}

export async function loadKeyStatus(): Promise<KeyStatus> {
  const connection = await getConnection();
  if (connection.env_key_available) return { hasKey: true, localModels: [] };
  // Best-effort: if the local list can't be read, a local chat is merely asked
  // for a key it could have skipped — the prompt never blocks sending.
  const installed = await installedLocalModels().catch(() => null);
  return { hasKey: false, localModels: installed?.models.map((model) => model.id) ?? [] };
}

/** True only when it is known that `model` is hosted and no key resolves. An
 *  unknown status never prompts; the 401 recovery card still covers that case. */
export function needsApiKey(status: KeyStatus | null, model: string | undefined): boolean {
  return !!status && !status.hasKey && !(model !== undefined && status.localModels.includes(model));
}
