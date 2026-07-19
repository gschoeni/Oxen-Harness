// A controllable mock of `src/lib/ipc`. Test files swap the real module for this
// with `vi.mock("../../lib/ipc", () => import("../../test/ipcMock"))`, then drive
// behavior via the exported `vi.fn`s (e.g. `runTurn.mockResolvedValueOnce`) and
// simulate backend events with `emit("token", "...")`.

import { vi } from "vitest";
import type {
  CatalogModel,
  CloudModel,
  CodeReviewRunResult,
  CompressionMode,
  ConnectionView,
  HardwareProfile,
  HfHit,
  InstalledView,
  OxenModelHit,
  Project,
  RuntimeStatus,
  StartProjectInput,
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
  tokens_used: 0,
  context_tokens: 0,
  context_window: 128000,
  compression_mode: "off",
};

export const sampleHardware: HardwareProfile = {
  ram_bytes: 16_000_000_000,
  vram_bytes: null,
  accelerator: "metal",
  chip_label: "Apple M2",
  usable_budget: 12_000_000_000,
};

export const sampleRuntime: RuntimeStatus = {
  binary: "/Users/dev/.oxen-harness/runtime/llama.cpp/llama-b10002/llama-server",
  source: "managed",
  managed_version: "b10002",
  can_manage: true,
};

export const sampleInstalled: InstalledView = {
  models: [
    {
      id: "qwen3-8b-q4-k-m",
      display: "Qwen3 8B · Q4_K_M",
      params: "8B",
      quant: "Q4_K_M",
      context: 40960,
      size_bytes: 5_000_000_000,
      origin: { kind: "huggingface", repo: "bartowski/x", file: "x.gguf", revision: "main" },
    },
  ],
  total_disk_bytes: 5_000_000_000,
  dir: "/Users/dev/.oxen-harness/models",
  runtime: sampleRuntime,
  disk_total: 500_000_000_000,
  disk_free: 220_000_000_000,
};

export const sampleCloudModels: CloudModel[] = [
  { id: "claude-opus-4-8", name: "Claude Opus 4.8", selected: true },
  { id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6", selected: false },
];

export const sampleOxenHits: OxenModelHit[] = [
  {
    id: "claude-sonnet-4-6",
    name: "Claude Sonnet 4.6",
    developer: "Anthropic",
    summary: "Balanced performance and speed",
    description: "Anthropic's **balanced** model.",
    endpoint: "/chat/completions",
    pricing: { input_cost_per_token: 3e-6, output_cost_per_token: 1.5e-5 },
    inputs: ["text", "image"],
    outputs: ["text"],
    context_length: 1_000_000,
    max_output_tokens: 64_000,
  },
  {
    id: "muse-spark-1-1",
    name: "Muse Spark 1.1",
    developer: "Muse",
    summary: "Fast and cheap",
    description: "A speedy little model.",
    endpoint: "/chat/completions",
    pricing: { input_cost_per_token: 2.5e-7, output_cost_per_token: 1e-6 },
    inputs: ["text"],
    outputs: ["text"],
    context_length: null,
    max_output_tokens: null,
  },
  {
    id: "pix-gen",
    name: "Pix Gen",
    developer: "Pix",
    summary: "Text-to-image",
    description: "",
    endpoint: "/images/generate",
    pricing: null,
    inputs: ["text"],
    outputs: ["image"],
    context_length: null,
    max_output_tokens: null,
  },
];

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
export const totalTokensUsed = vi.fn(async () => 0);
export const totalCostUsd = vi.fn(async () => null as number | null);
export const modelUsageBreakdown = vi.fn(async () => ({
  rows: [] as { model: string; source: "oxen_cloud" | "unpriced"; prompt_tokens: number; completion_tokens: number; cost_usd: number | null }[],
  total_cost_usd: 0,
  prompt_tokens: 0,
  completion_tokens: 0,
  has_unpriced_usage: false,
}));
export const sessionCost = vi.fn(async () => null as number | null);
export const dailyUsage = vi.fn(async () => [] as { date: string; prompt_tokens: number; completion_tokens: number }[]);
export const sessionMessages = vi.fn(async () => [] as unknown[]);
export const toolDefinitions = vi.fn(async () => [] as unknown[]);
export const listTools = vi.fn(async () => [] as unknown[]);
export const setToolEnabled = vi.fn(async () => {});
export const setToolDescription = vi.fn(async () => {});
export const getCompressionMode = vi.fn(async (): Promise<CompressionMode> => "off");
export const setCompressionMode = vi.fn(async (mode: CompressionMode) => ({
  ...sampleSession,
  compression_mode: mode,
}));
export const totalTokensSaved = vi.fn(async () => 0);
export const addCustomTool = vi.fn(async () => {});
export const removeCustomTool = vi.fn(async () => {});
export const listSkills = vi.fn(async () => [] as unknown[]);
export const saveSkill = vi.fn(async () => {});
export const deleteSkill = vi.fn(async () => {});
export const setSkillEnabled = vi.fn(async () => {});
export const exportFinetuning = vi.fn(async () => 0);
export const pickExportPath = vi.fn(async () => null as string | null);
export const importSourcesScan = vi.fn(
  async () => [] as { source: string; available: number; imported: number }[],
);
export const importExternal = vi.fn(async () => ({ imported: 0, updated: 0, skipped: 0 }));
export const attachmentDataUri = vi.fn(async () => "data:image/png;base64,AAAA");
export const newSession = vi.fn(async () => ({ ...sampleSession, session_id: "new-session-id" }));
export const resumeSession = vi.fn(async () => emptyView);
export const deleteSession = vi.fn(async () => {});
export const listProjects = vi.fn(async () => [] as Project[]);
export const openProject = vi.fn(async (path: string) => ({
  path,
  name: path,
  description: "",
  instructions: "",
  context: [],
  session_count: 0,
  active: true,
  last_used_at: null,
}));
export const startProject = vi.fn(async (input: StartProjectInput): Promise<Project> => ({
  path: input.createDirectory ? `${input.directory}/${input.name}` : input.directory,
  name: input.name,
  description: input.description,
  instructions: "",
  context: [],
  session_count: 0,
  active: true,
  last_used_at: null,
}));
export const updateProject = vi.fn(async (path: string, name: string, description: string, instructions: string): Promise<Project> => ({
  path, name, description, instructions, context: [], session_count: 0, active: true, last_used_at: null,
}));
export const deleteProject = vi.fn(async () => {});
export const addProjectContext = vi.fn(async (path: string, contextPaths: string[]): Promise<Project> => ({
  path, name: "Demo", description: "", instructions: "", session_count: 0, active: true, last_used_at: null,
  context: contextPaths.map((source) => ({ path: source, name: source.split("/").pop() ?? source, kind: "text" as const, size_bytes: 42 })),
}));
export const removeProjectContext = vi.fn(async (path: string): Promise<Project> => ({
  path, name: "Demo", description: "", instructions: "", context: [], session_count: 0, active: true, last_used_at: null,
}));
export const setActiveProject = vi.fn(async () => {});
export const selectCloudModelForNewChats = vi.fn(async () => {});
export const getDefaultProjectLocation = vi.fn(async () => null as string | null);
export const setDefaultProjectLocation = vi.fn(async (path: string) => path);
export const pickFolder = vi.fn(async () => null as string | null);
export const pickProjectParent = vi.fn(async () => null as string | null);
export const pickProjectContext = vi.fn(async () => [] as string[]);
export const runTurn = vi.fn(async () => "Done.");
export const runLoop = vi.fn(async () => ({ succeeded: true, iterations: 1, summary: "Loop complete." }));
export const listLoops = vi.fn(async () => []);
export const loopsPath = vi.fn(async () => "/tmp/loops");
export const getLoop = vi.fn();
export const saveLoop = vi.fn(async () => {});
export const importLoop = vi.fn();
export const exportLoop = vi.fn(async (_name: string, path: string) => path);
export const removeLoop = vi.fn(async () => {});
export const pickLoopImportPath = vi.fn(async () => null as string | null);
export const pickLoopExportPath = vi.fn(async () => null as string | null);
export const themeLocation = vi.fn(async () => null as string | null);
export const setThemeLocation = vi.fn(async () => {});
export const retryTurn = vi.fn(async () => "Done.");
export const cancelTurn = vi.fn(async () => {});
export const configureOxenKey = vi.fn(async () => {});
export const onToken = listener("token");
export const onTool = listener("tool");
export const onCompression = listener("compression");
export const onRetry = listener("retry");
export const onQuestion = listener("question");
export const onFileDrop = listener("fileDrop");
export const pickAttachments = vi.fn(async () => [] as string[]);
export const answerQuestion = vi.fn(async () => {});
export const onApprovalRequest = listener("approvalRequest");
export const onApproval = listener("approval");
export const answerApproval = vi.fn(async () => {});

// ---- permissions (Settings → Permissions) -----------------------------------
const emptyRules = { mode: null, allow: [], allow_exact: [], deny: [] };
export const getPermissions = vi.fn(async () => ({
  mode: "relaxed",
  global: { ...emptyRules },
  project: { ...emptyRules },
  project_path: "/tmp/proj",
}));
export const setPermissionMode = vi.fn(async () => {});
export const addPermissionRule = vi.fn(async () => {});
export const removePermissionRule = vi.fn(async () => {});

// ---- live preview ------------------------------------------------------------
export const onPreviewStatus = listener("previewStatus");
export const onPreviewConsole = listener("previewConsole");
export const previewAttach = vi.fn(async () => {});
export const previewDetach = vi.fn(async () => {});
export const previewReload = vi.fn(async () => {});
export const previewStop = vi.fn(async () => {});
export const previewOpenExternal = vi.fn(async () => {});
export const previewStatus = vi.fn(async () => null);
export const previewStatuses = vi.fn(async () => [] as unknown[]);
export const previewRestart = vi.fn(async () => {});
export const getPreviewPrefs = vi.fn(async () => ({ auto_verify: true }));
export const setPreviewAutoVerify = vi.fn(async () => {});

// ---- workspace files (Files tree + Editor dock) -------------------------------
export const fsListDir = vi.fn(
  async (_root: string, _path: string) => [] as { name: string; path: string; is_dir: boolean }[],
);
export const fsReadFile = vi.fn(async (_root: string, _path: string) => ({
  content: "",
  truncated: false,
  size: 0,
}));
export const fsWriteFile = vi.fn(async (_root: string, _path: string, _content: string) => {});
export const fsCreateEntry = vi.fn(async (_root: string, _path: string, _isDir: boolean) => {});
export const emptyDatasetPage = {
  columns: [] as { name: string; dtype: string; kind: string }[],
  rows: [] as (string | number | boolean | null)[][],
  rowIds: [] as number[],
  totalRows: 0,
  fileSize: 0,
  format: "csv",
  elapsedMs: 0,
  editable: true,
  mtimeMs: 0,
};
export const datasetQuery = vi.fn(async (_root: string, _path: string, _req: unknown) => ({
  ...emptyDatasetPage,
}));
export const datasetWriteCell = vi.fn(
  async (
    _root: string,
    _path: string,
    _row: number,
    _column: string,
    _value: unknown,
    _expectedMtimeMs?: number,
  ) => 0,
);
export const fsWatch = vi.fn(async (_root: string) => {});
export const fsUnwatch = vi.fn(async (_root: string) => {});
export const onFsChanged = listener("fsChanged");

// ---- link browser ------------------------------------------------------------
export const onBrowserOpen = listener("browserOpen");
export const browserAttach = vi.fn(async () => {});
export const browserDetach = vi.fn(async () => {});
export const browserClose = vi.fn(async () => {});
export const browserReload = vi.fn(async () => {});
export const openExternal = vi.fn(async () => {});

export const getConnection = vi.fn(async () => sampleConnection);
export const setConnection = vi.fn(async () => ({ ...sampleSession, session_id: "reconnected" }));
export const configureBraveKey = vi.fn(async () => {});

export const installedLocalModels = vi.fn(async () => sampleInstalled);
export const detectHardware = vi.fn(async () => sampleHardware);
export const runtimeStatus = vi.fn(async () => sampleRuntime);
export const installRuntime = vi.fn(async () => {});
export const listModelCatalog = vi.fn(async (): Promise<CatalogModel[]> => []);
export const resolveHfModel = vi.fn((_input: string): Promise<CatalogModel> => {
  throw new Error("not mocked");
});
export const searchHfModels = vi.fn(async (): Promise<HfHit[]> => []);
export const hfTokenPresent = vi.fn(async () => false);
export const setHfToken = vi.fn(async () => {});
export const downloadModel = vi.fn(async () => {});
export const installLlama = vi.fn(async () => {});
export const removeModel = vi.fn(async () => {});
export const useLocalModel = vi.fn(async () => ({ ...sampleSession, session_id: "local-session" }));
export const onModelProgress = listener("modelProgress");
export const onRuntimeInstall = listener("runtimeInstall");
export const onLocalStatus = listener("localStatus");
export const onLlamaInstall = listener("llamaInstall");

// ---- cloud models ----------------------------------------------------------
export const listCloudModels = vi.fn(async () => sampleCloudModels);
export const addCloudModel = vi.fn(async () => sampleCloudModels);
export const removeCloudModel = vi.fn(async () => sampleCloudModels);
export const searchOxenModels = vi.fn(async () => sampleOxenHits);
export const setModel = vi.fn(async () => ({ ...sampleSession, session_id: "model-switched" }));

export const runCodeReview = vi.fn(async (): Promise<CodeReviewRunResult> => ({
  status: "ok",
  user: "Run a code review of the uncommitted changes in this workspace.",
  assistant: "## Code review: no findings\n\nNothing qualifying survived verification.",
  findings: 0,
  tokens_used: 4200,
}));

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
  deleteSession.mockReset().mockResolvedValue(undefined);
  listProjects.mockReset().mockResolvedValue([]);
  openProject.mockReset().mockImplementation(async (path: string) => ({
    path,
    name: path,
    description: "",
    instructions: "",
    context: [],
    session_count: 0,
    active: true,
    last_used_at: null,
  }));
  startProject.mockReset().mockImplementation(async (input: StartProjectInput): Promise<Project> => ({
    path: input.createDirectory ? `${input.directory}/${input.name}` : input.directory,
    name: input.name,
    description: input.description,
    instructions: "",
    context: [],
    session_count: 0,
    active: true,
    last_used_at: null,
  }));
  updateProject.mockReset().mockImplementation(async (path: string, name: string, description: string, instructions: string): Promise<Project> => ({
    path, name, description, instructions, context: [], session_count: 0, active: true, last_used_at: null,
  }));
  deleteProject.mockReset().mockResolvedValue(undefined);
  addProjectContext.mockReset().mockImplementation(async (path: string, contextPaths: string[]): Promise<Project> => ({
    path, name: "Demo", description: "", instructions: "", session_count: 0, active: true, last_used_at: null,
    context: contextPaths.map((source) => ({ path: source, name: source.split("/").pop() ?? source, kind: "text" as const, size_bytes: 42 })),
  }));
  removeProjectContext.mockReset().mockImplementation(async (path: string): Promise<Project> => ({
    path, name: "Demo", description: "", instructions: "", context: [], session_count: 0, active: true, last_used_at: null,
  }));
  setActiveProject.mockReset().mockResolvedValue(undefined);
  selectCloudModelForNewChats.mockReset().mockResolvedValue(undefined);
  getDefaultProjectLocation.mockReset().mockResolvedValue(null);
  setDefaultProjectLocation.mockReset().mockImplementation(async (path: string) => path);
  pickFolder.mockReset().mockResolvedValue(null);
  pickProjectParent.mockReset().mockResolvedValue(null);
  pickProjectContext.mockReset().mockResolvedValue([]);
  runTurn.mockReset().mockResolvedValue("Done.");
  runLoop.mockReset().mockResolvedValue({ succeeded: true, iterations: 1, summary: "Loop complete." });
  listLoops.mockReset().mockResolvedValue([]);
  loopsPath.mockReset().mockResolvedValue("/tmp/loops");
  saveLoop.mockReset().mockResolvedValue(undefined);
  exportLoop.mockReset().mockImplementation(async (_name: string, path: string) => path);
  removeLoop.mockReset().mockResolvedValue(undefined);
  pickLoopImportPath.mockReset().mockResolvedValue(null);
  pickLoopExportPath.mockReset().mockResolvedValue(null);
  themeLocation.mockReset().mockResolvedValue(null);
  setThemeLocation.mockReset().mockResolvedValue(undefined);
  retryTurn.mockReset().mockResolvedValue("Done.");
  cancelTurn.mockReset().mockResolvedValue(undefined);
  configureOxenKey.mockReset().mockResolvedValue(undefined);
  pickAttachments.mockReset().mockResolvedValue([]);
  answerQuestion.mockReset().mockResolvedValue(undefined);
  answerApproval.mockReset().mockResolvedValue(undefined);
  setPermissionMode.mockReset().mockResolvedValue(undefined);
  addPermissionRule.mockReset().mockResolvedValue(undefined);
  removePermissionRule.mockReset().mockResolvedValue(undefined);
  getPermissions.mockReset().mockResolvedValue({
    mode: "relaxed",
    global: { mode: null, allow: [], allow_exact: [], deny: [] },
    project: { mode: null, allow: [], allow_exact: [], deny: [] },
    project_path: "/tmp/proj",
  });
  previewAttach.mockReset().mockResolvedValue(undefined);
  previewDetach.mockReset().mockResolvedValue(undefined);
  previewReload.mockReset().mockResolvedValue(undefined);
  previewStop.mockReset().mockResolvedValue(undefined);
  previewOpenExternal.mockReset().mockResolvedValue(undefined);
  previewStatus.mockReset().mockResolvedValue(null);
  previewStatuses.mockReset().mockResolvedValue([]);
  previewRestart.mockReset().mockResolvedValue(undefined);
  getPreviewPrefs.mockReset().mockResolvedValue({ auto_verify: true });
  setPreviewAutoVerify.mockReset().mockResolvedValue(undefined);
  fsListDir.mockReset().mockResolvedValue([]);
  fsReadFile.mockReset().mockResolvedValue({ content: "", truncated: false, size: 0 });
  fsWriteFile.mockReset().mockResolvedValue(undefined);
  fsCreateEntry.mockReset().mockResolvedValue(undefined);
  datasetQuery.mockReset().mockResolvedValue({ ...emptyDatasetPage });
  datasetWriteCell.mockReset().mockResolvedValue(0);
  browserAttach.mockReset().mockResolvedValue(undefined);
  browserDetach.mockReset().mockResolvedValue(undefined);
  browserClose.mockReset().mockResolvedValue(undefined);
  browserReload.mockReset().mockResolvedValue(undefined);
  openExternal.mockReset().mockResolvedValue(undefined);
  getConnection.mockReset().mockResolvedValue(sampleConnection);
  setConnection.mockReset().mockResolvedValue({ ...sampleSession, session_id: "reconnected" });
  configureBraveKey.mockReset().mockResolvedValue(undefined);
  installedLocalModels.mockReset().mockResolvedValue(sampleInstalled);
  detectHardware.mockReset().mockResolvedValue(sampleHardware);
  runtimeStatus.mockReset().mockResolvedValue(sampleRuntime);
  installRuntime.mockReset().mockResolvedValue(undefined);
  listModelCatalog.mockReset().mockResolvedValue([]);
  searchHfModels.mockReset().mockResolvedValue([]);
  hfTokenPresent.mockReset().mockResolvedValue(false);
  setHfToken.mockReset().mockResolvedValue(undefined);
  downloadModel.mockReset().mockResolvedValue(undefined);
  installLlama.mockReset().mockResolvedValue(undefined);
  removeModel.mockReset().mockResolvedValue(undefined);
  useLocalModel.mockReset().mockResolvedValue({ ...sampleSession, session_id: "local-session" });
  listCloudModels.mockReset().mockResolvedValue(sampleCloudModels);
  addCloudModel.mockReset().mockResolvedValue(sampleCloudModels);
  removeCloudModel.mockReset().mockResolvedValue(sampleCloudModels);
  searchOxenModels.mockReset().mockResolvedValue(sampleOxenHits);
  setModel.mockReset().mockResolvedValue({ ...sampleSession, session_id: "model-switched" });
  listThemes.mockReset().mockResolvedValue(sampleThemes);
  activeTheme.mockReset().mockResolvedValue(sampleTheme);
  useTheme.mockReset().mockResolvedValue(sampleTheme);
  importTheme.mockReset().mockResolvedValue(sampleTheme);
  exportTheme.mockReset().mockResolvedValue('name = "Oregon Trail"');
  removeTheme.mockReset().mockResolvedValue(undefined);
  newTheme.mockReset().mockResolvedValue(sampleTheme);
  totalTokensUsed.mockReset().mockResolvedValue(0);
  totalCostUsd.mockReset().mockResolvedValue(null);
  modelUsageBreakdown.mockReset().mockResolvedValue({
    rows: [], total_cost_usd: 0, prompt_tokens: 0, completion_tokens: 0, has_unpriced_usage: false,
  });
  dailyUsage.mockReset().mockResolvedValue([]);
  sessionMessages.mockReset().mockResolvedValue([]);
  toolDefinitions.mockReset().mockResolvedValue([]);
  listTools.mockReset().mockResolvedValue([]);
  setToolEnabled.mockReset().mockResolvedValue(undefined);
  setToolDescription.mockReset().mockResolvedValue(undefined);
  getCompressionMode.mockReset().mockResolvedValue("off");
  setCompressionMode
    .mockReset()
    .mockImplementation(async (mode: CompressionMode) => ({ ...sampleSession, compression_mode: mode }));
  totalTokensSaved.mockReset().mockResolvedValue(0);
  addCustomTool.mockReset().mockResolvedValue(undefined);
  removeCustomTool.mockReset().mockResolvedValue(undefined);
  listSkills.mockReset().mockResolvedValue([]);
  saveSkill.mockReset().mockResolvedValue(undefined);
  deleteSkill.mockReset().mockResolvedValue(undefined);
  setSkillEnabled.mockReset().mockResolvedValue(undefined);
  attachmentDataUri.mockReset().mockResolvedValue("data:image/png;base64,AAAA");
  runCodeReview.mockReset().mockResolvedValue({
    status: "ok",
    user: "Run a code review of the uncommitted changes in this workspace.",
    assistant: "## Code review: no findings\n\nNothing qualifying survived verification.",
    findings: 0,
    tokens_used: 4200,
  });
  [
    onToken,
    onTool,
    onCompression,
    onRetry,
    onQuestion,
    onApprovalRequest,
    onApproval,
    onFileDrop,
    onModelProgress,
    onRuntimeInstall,
    onLocalStatus,
    onLlamaInstall,
    onPreviewStatus,
    onPreviewConsole,
    onBrowserOpen,
  ].forEach((fn) => fn.mockClear());
}
