// Test helpers. Imported by test files (after their `vi.mock` of lib/ipc) so the
// store reset here touches the same store instance bound to the mocked IPC.
import { initFeatureFlags, type FeatureFlags } from "../lib/features";
import { useStore } from "../lib/store";
import { resetUiState } from "../lib/uiState";
import { loadFeatureFlags, resetIpc } from "./ipcMock";

/** Reset IPC mocks, UI prefs, localStorage, and the global store to a clean slate. */
export function resetAll() {
  // Streamed tokens buffer outside the store; a burst left pending by one
  // test must not land in the next test's threads.
  useStore.getState().flushTokens();
  resetIpc();
  resetUiState();
  localStorage.clear();
  useStore.setState({
    theme: null,
    heroGame: null,
    gameDockOpen: false,
    session: null,
    sessions: [],
    projects: [],
    homeOpen: true,
    projectHomePath: null,
    threadsSnapshot: null,
    infos: {},
    threads: {},
    downloads: {},
    downloadsRev: 0,
    sessionUsage: {},
    compression: {},
    runStatus: {},
    fleets: {},
    agentView: {},
    codeReview: {},
    queues: {},
    canvases: {},
    activeCanvas: {},
    canvasWriting: {},
    media: {},
    mediaUploads: {},
    mediaPrefs: null,
    mediaFocus: null,
    pendingAttachments: [],
    previews: {},
    previewClosed: {},
    previewErrors: {},
    rightTab: {},
    workContexts: {},
    browserUrl: null,
    leftTab: null,
    chatTabs: {},
    historyOpen: false,
    filesReveal: null,
    editorTabs: {},
    fsChange: null,
    snippets: {},
    gitStates: {},
    editorWrap: false,
    settingsOpen: false,
    settingsPage: "connection",
    question: null,
    approvals: {},
  });
}

/** Boot the release flags as the host would report them; any left out are off.
 *  Flags outlive `resetAll`, so a file that turns one on resets it per test. */
export async function setFeatureFlags(on: Partial<FeatureFlags> = {}) {
  loadFeatureFlags.mockResolvedValueOnce({
    workbench_customization: false,
    workflows: false,
    advanced_settings: false,
    ...on,
  });
  await initFeatureFlags();
}
