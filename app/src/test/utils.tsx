// Test helpers. Imported by test files (after their `vi.mock` of lib/ipc) so the
// store reset here touches the same store instance bound to the mocked IPC.
import { useStore } from "../lib/store";
import { resetIpc } from "./ipcMock";

/** Reset IPC mocks, localStorage, and the global store to a clean slate. */
export function resetAll() {
  resetIpc();
  localStorage.clear();
  useStore.setState({
    theme: null,
    heroGame: null,
    gameDockOpen: false,
    session: null,
    sessions: [],
    projects: [],
    projectsOpen: true,
    projectHomePath: null,
    infos: {},
    threads: {},
    sessionUsage: {},
    compression: {},
    runStatus: {},
    queues: {},
    canvases: {},
    activeCanvas: {},
    canvasWriting: {},
    previews: {},
    previewClosed: {},
    previewErrors: {},
    rightTab: {},
    browserUrl: null,
    leftTab: null,
    editorFiles: {},
    snippets: {},
    settingsOpen: false,
    settingsPage: "connection",
    question: null,
  });
}
