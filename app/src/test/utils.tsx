// Test helpers. Imported by test files (after their `vi.mock` of lib/ipc) so the
// store reset here touches the same store instance bound to the mocked IPC.
import { useStore } from "../lib/store";
import { resetUiState } from "../lib/uiState";
import { resetIpc } from "./ipcMock";

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
    ledger: null,
    ledgerGit: {},
    infos: {},
    threads: {},
    downloads: {},
    downloadsRev: 0,
    sessionUsage: {},
    compression: {},
    runStatus: {},
    trailDust: {},
    trailActivity: {},
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
