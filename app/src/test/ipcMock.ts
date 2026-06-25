// A controllable mock of `src/lib/ipc`. Test files swap the real module for this
// with `vi.mock("../../lib/ipc", () => import("../../test/ipcMock"))`, then drive
// behavior via the exported `vi.fn`s (e.g. `runTurn.mockResolvedValueOnce`) and
// simulate backend events with `emit("token", "...")`.

import { vi } from "vitest";
import type {
  ConnectionView,
  ModelsView,
  Project,
  SessionInfo,
  SessionView,
  Theme,
  ThemeSummary,
} from "../lib/types";

// ---- event plumbing --------------------------------------------------------

type Handler = (arg: unknown) => void;
const handlers: Record<string, Handler[]> = {};

function listener(event: string) {
  return vi.fn((h: Handler) => {
    (handlers[event] ??= []).push(h);
    return Promise.resolve(() => {
      handlers[event] = (handlers[event] || []).filter((x) => x !== h);
    });
  });
}

/** Fire a simulated backend event to all current subscribers. */
export function emit(event: string, payload: unknown) {
  (handlers[event] || []).forEach((h) => h(payload));
}

// ---- sample fixtures -------------------------------------------------------

export const sampleTheme: Theme = {
  meta: { name: "Oregon Trail", author: "", description: "A dusty wagon trail." },
  palette: {
    title: "#e8d9b5",
    primary: "#c08457",
    secondary: "#8a7a5c",
    text: "#f6f4ef",
    muted: "#888379",
    danger: "#ff5d57",
    link: "#6aa6ff",
    background: "#1a1917",
    surface: "#211f1d",
    border: "#332f2c",
  },
  voice: {
    prompt_icon: "🐂",
    prompt_label: "trail ❯",
    spinner_glyphs: ["⠋", "⠙"],
    thinking: ["Fording the river"],
    tool_verbs: { default: ["Working the trail"] },
    deaths: ["You have died of dysentery."],
    wordmark: "OXEN TRAIL",
    pre_tagline: "～ The ～",
    subtitle: "an open source agentic coding trail · powered by Oxen.ai",
    flavor_top: [["Departing", "Independence, Missouri · 1848"]],
    flavor_bottom: [
      ["Date", "March 21, 1848"],
      ["Weather", "warm"],
    ],
    bottom_hint: "Press RETURN to size up the situation",
  },
  style: {
    font_display: '"PixelHead", monospace',
    font_body: "-apple-system, sans-serif",
    font_mono: '"PixelRead", monospace',
    display_transform: "uppercase",
    display_spacing: "0.02em",
    radius: "3px",
    border_width: "2px",
    shadow: "pixel",
    hero: "pixel",
    scene: "trail",
  },
};

export const sampleSession: SessionInfo = {
  model: "claude-opus-4-8",
  workspace: "/Users/dev/project",
  session_id: "current-session-id",
};

export const sampleModels: ModelsView = {
  models: [
    {
      id: "qwen2.5-coder-7b",
      display: "Qwen2.5 Coder 7B",
      params: "7B",
      quant: "Q4_K_M",
      context: 32768,
      note: "fast",
      installed: false,
      size_bytes: 4_700_000_000,
      size_is_actual: false,
    },
    {
      id: "llama-3.2-3b",
      display: "Llama 3.2 3B",
      params: "3B",
      quant: "Q4_K_M",
      context: 8192,
      note: "tiny",
      installed: true,
      size_bytes: 2_000_000_000,
      size_is_actual: true,
    },
  ],
  total_disk_bytes: 2_000_000_000,
  dir: "/Users/dev/.oxen-harness/models",
  llama_installed: true,
  can_auto_install: true,
  install_hint: "brew install llama.cpp",
};

export const sampleThemes: ThemeSummary[] = [
  {
    name: "Oregon Trail",
    slug: "oregon-trail",
    description: "A dusty wagon trail.",
    builtin: true,
    installed: false,
    active: true,
  },
  {
    name: "Midnight",
    slug: "midnight",
    description: "Sleek and dark.",
    builtin: true,
    installed: false,
    active: false,
  },
  {
    name: "My Custom",
    slug: "my-custom",
    description: "A user theme.",
    builtin: false,
    installed: true,
    active: false,
  },
];

export const sampleConnection: ConnectionView = {
  host: "hub.oxen.ai",
  api_key: "sk-test",
  brave_api_key: "",
  default_host: "hub.oxen.ai",
  env_key_available: true,
};

const emptyView: SessionView = { info: sampleSession, messages: [], running: false };

// ---- mocked command + event functions --------------------------------------

export const sessionInfo = vi.fn(async () => sampleSession);
export const listSessions = vi.fn(async () => []);
export const newSession = vi.fn(async () => ({ ...sampleSession, session_id: "new-session-id" }));
export const resumeSession = vi.fn(async () => emptyView);
export const listProjects = vi.fn(async () => [] as Project[]);
export const openProject = vi.fn(async (path: string) => ({
  path,
  name: path,
  session_count: 0,
  active: true,
}));
export const setActiveProject = vi.fn(async () => {});
export const pickFolder = vi.fn(async () => null as string | null);
export const runTurn = vi.fn(async () => "Done.");
export const onToken = listener("token");
export const onTool = listener("tool");
export const onQuestion = listener("question");
export const onFileDrop = listener("fileDrop");
export const pickAttachments = vi.fn(async () => [] as string[]);
export const answerQuestion = vi.fn(async () => {});

export const getConnection = vi.fn(async () => sampleConnection);
export const setConnection = vi.fn(async () => ({ ...sampleSession, session_id: "reconnected" }));
export const configureBraveKey = vi.fn(async () => {});

export const listModels = vi.fn(async () => sampleModels);
export const installLlama = vi.fn(async () => {});
export const pullModel = vi.fn(async () => {});
export const removeModel = vi.fn(async () => {});
export const useLocalModel = vi.fn(async () => ({ ...sampleSession, session_id: "local-session" }));
export const onModelProgress = listener("modelProgress");
export const onLlamaInstall = listener("llamaInstall");

export const listThemes = vi.fn(async () => sampleThemes);
export const activeTheme = vi.fn(async () => sampleTheme);
export const useTheme = vi.fn(async () => sampleTheme);
export const importTheme = vi.fn(async () => sampleTheme);
export const exportTheme = vi.fn(async () => "name = \"Oregon Trail\"");
export const removeTheme = vi.fn(async () => {});
export const newTheme = vi.fn(async () => sampleTheme);

/** Restore default implementations + clear call history and event subscribers. */
export function resetIpc() {
  for (const k of Object.keys(handlers)) delete handlers[k];
  sessionInfo.mockReset().mockResolvedValue(sampleSession);
  listSessions.mockReset().mockResolvedValue([]);
  newSession.mockReset().mockResolvedValue({ ...sampleSession, session_id: "new-session-id" });
  resumeSession.mockReset().mockResolvedValue(emptyView);
  listProjects.mockReset().mockResolvedValue([]);
  openProject.mockReset().mockImplementation(async (path: string) => ({
    path,
    name: path,
    session_count: 0,
    active: true,
  }));
  setActiveProject.mockReset().mockResolvedValue(undefined);
  pickFolder.mockReset().mockResolvedValue(null);
  runTurn.mockReset().mockResolvedValue("Done.");
  pickAttachments.mockReset().mockResolvedValue([]);
  answerQuestion.mockReset().mockResolvedValue(undefined);
  getConnection.mockReset().mockResolvedValue(sampleConnection);
  setConnection.mockReset().mockResolvedValue({ ...sampleSession, session_id: "reconnected" });
  configureBraveKey.mockReset().mockResolvedValue(undefined);
  listModels.mockReset().mockResolvedValue(sampleModels);
  installLlama.mockReset().mockResolvedValue(undefined);
  pullModel.mockReset().mockResolvedValue(undefined);
  removeModel.mockReset().mockResolvedValue(undefined);
  useLocalModel.mockReset().mockResolvedValue({ ...sampleSession, session_id: "local-session" });
  listThemes.mockReset().mockResolvedValue(sampleThemes);
  activeTheme.mockReset().mockResolvedValue(sampleTheme);
  useTheme.mockReset().mockResolvedValue(sampleTheme);
  importTheme.mockReset().mockResolvedValue(sampleTheme);
  exportTheme.mockReset().mockResolvedValue('name = "Oregon Trail"');
  removeTheme.mockReset().mockResolvedValue(undefined);
  newTheme.mockReset().mockResolvedValue(sampleTheme);
  [onToken, onTool, onQuestion, onFileDrop, onModelProgress, onLlamaInstall].forEach((fn) =>
    fn.mockClear(),
  );
}
