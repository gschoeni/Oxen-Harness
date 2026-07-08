import {
  Check,
  Cloud,
  Cpu,
  Eye,
  EyeOff,
  FolderOpen,
  GraduationCap,
  Link2,
  Moon,
  Palette,
  Plus,
  ScrollText,
  SearchCode,
  Shrink,
  Star,
  Sun,
  Trash2,
  Wrench,
  X,
} from "lucide-react";
import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { Button } from "../../components/ui";
import {
  addCloudModel,
  getConnection,
  removeCloudModel,
  setConnection,
} from "../../lib/ipc";
import { useActiveProject, useStore } from "../../lib/store";
import type { SettingsPage } from "../../lib/types";
import { LocalSetup } from "../models/LocalSetup";
import { ThemesPanel } from "../themes/ThemesPanel";
import { ToolsPage } from "../tools/ToolsPage";
import { SkillsPage } from "../skills/SkillsPage";
import { CodeReviewPage } from "./CodeReviewPage";
import { CompressionPage } from "./CompressionPage";
import { LogsPage } from "../logs/LogsPage";
import "./settings.css";

/** The settings sidebar entries, in display order. Each maps a page key to its
 *  icon, label, and a one-line description shown under the label in the rail. */
const NAV: { page: SettingsPage; icon: ReactNode; label: string; blurb: string }[] = [
  { page: "connection", icon: <Link2 size={18} />, label: "Connection", blurb: "Oxen endpoint & API keys" },
  { page: "cloud-models", icon: <Cloud size={18} />, label: "Cloud models", blurb: "Hosted model catalog" },
  { page: "local-models", icon: <Cpu size={18} />, label: "Local models", blurb: "Download & run on-device" },
  { page: "tools", icon: <Wrench size={18} />, label: "Tools", blurb: "What the agent can do" },
  { page: "skills", icon: <GraduationCap size={18} />, label: "Skills", blurb: "Reusable workflows it can learn" },
  { page: "code-review", icon: <SearchCode size={18} />, label: "Code review", blurb: "The find → verify → report pipeline" },
  { page: "compression", icon: <Shrink size={18} />, label: "Compression", blurb: "Shrink stale context on the wire" },
  { page: "appearance", icon: <Palette size={18} />, label: "Appearance", blurb: "Theme & light/dark" },
  { page: "logs", icon: <ScrollText size={18} />, label: "Training data", blurb: "Curate chats & export for fine-tuning" },
];

const TITLE: Record<SettingsPage, string> = {
  connection: "Connection",
  "cloud-models": "Cloud models",
  "local-models": "Local models",
  tools: "Tools",
  skills: "Skills",
  "code-review": "Code review",
  compression: "Compression",
  appearance: "Appearance",
  logs: "Training data",
};

/** The full-screen settings surface: a left rail of subpages and a content pane.
 *  Replaces the old stacked settings modal — each concern (connection, models,
 *  tools, appearance, logs) now gets its own dedicated page. */
export function Settings() {
  const page = useStore((s) => s.settingsPage);
  const setPage = useStore((s) => s.setSettingsPage);
  const close = useStore((s) => s.setSettingsOpen);
  const project = useActiveProject();

  // Esc closes the whole surface.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [close]);

  return (
    <div className="settings-overlay">
      <div className="settings-shell" role="dialog" aria-modal="true" aria-label="Settings">
        <aside className="settings-rail" data-tauri-drag-region>
          <div className="settings-rail-title">Settings</div>
          {/* The context anything project-scoped (e.g. project skills) applies
              to. Global settings live in ~/.oxen-harness regardless. */}
          <div
            className="settings-rail-project"
            title={project ? project.path : "Open a project to use project-scoped settings"}
          >
            <FolderOpen size={13} />
            <span className="settings-rail-project-name">
              {project ? project.name : "No project open"}
            </span>
          </div>
          <nav className="settings-rail-nav">
            {NAV.map((item) => (
              <button
                key={item.page}
                className={`settings-rail-item ${page === item.page ? "active" : ""}`}
                onClick={() => setPage(item.page)}
                aria-current={page === item.page}
              >
                <span className="settings-rail-icon">{item.icon}</span>
                <span className="settings-rail-text">
                  <span className="settings-rail-label">{item.label}</span>
                  <span className="settings-rail-blurb">{item.blurb}</span>
                </span>
              </button>
            ))}
          </nav>
        </aside>

        <section className="settings-main">
          <header className="settings-main-header" data-tauri-drag-region>
            <h2 className="settings-main-title">{TITLE[page]}</h2>
            <button className="settings-close" onClick={() => close(false)} aria-label="Close settings">
              <X size={20} />
            </button>
          </header>
          <div className="settings-main-body">
            {page === "connection" && <ConnectionSettings />}
            {page === "cloud-models" && <CloudModelsSettings />}
            {page === "local-models" && <LocalSetup />}
            {page === "tools" && <ToolsPage />}
            {page === "skills" && <SkillsPage />}
            {page === "code-review" && <CodeReviewPage />}
            {page === "compression" && <CompressionPage />}
            {page === "appearance" && <AppearanceSettings />}
            {page === "logs" && <LogsPage />}
          </div>
        </section>
      </div>
    </div>
  );
}

/** Editable Oxen API key + host, plus a read-only readout of the current session.
 *  Saving rebuilds the agent (which starts a fresh session on the new endpoint). */
function ConnectionSettings() {
  const session = useStore((s) => s.session);
  const adoptSession = useStore((s) => s.adoptSession);
  const refreshHistory = useStore((s) => s.refreshHistory);

  const [host, setHost] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [braveKey, setBraveKey] = useState("");
  const [defaultHost, setDefaultHost] = useState("hub.oxen.ai");
  const [envKey, setEnvKey] = useState(false);
  const [revealKey, setRevealKey] = useState(false);
  const [revealBrave, setRevealBrave] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<{ ok: boolean; msg: string } | null>(null);

  useEffect(() => {
    getConnection()
      .then((c) => {
        setHost(c.host);
        setApiKey(c.api_key);
        setBraveKey(c.brave_api_key);
        setDefaultHost(c.default_host);
        setEnvKey(c.env_key_available);
      })
      .catch(() => {})
      .finally(() => setLoaded(true));
  }, []);

  async function save() {
    setSaving(true);
    setStatus(null);
    try {
      const info = await setConnection(host.trim(), apiKey.trim(), braveKey.trim());
      adoptSession(info);
      refreshHistory();
      setStatus({ ok: true, msg: "Saved — started a fresh chat on the new settings." });
    } catch (e) {
      setStatus({ ok: false, msg: String(e) });
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="settings-page">
      <section className="settings-section">
        <div className="settings-label">Oxen connection</div>
        <div className="fields">
          <label className="field">
            <span className="field-name">Host</span>
            <input
              className="field-input"
              placeholder={defaultHost}
              value={host}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              onChange={(e) => setHost(e.target.value)}
            />
          </label>
          <label className="field">
            <span className="field-name">API key</span>
            <div className="field-with-action">
              <input
                className="field-input"
                type={revealKey ? "text" : "password"}
                placeholder={envKey ? "Using OXEN_API_KEY / oxen login" : "sk-…"}
                value={apiKey}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(e) => setApiKey(e.target.value)}
              />
              <button
                type="button"
                className="field-action"
                aria-label={revealKey ? "Hide API key" : "Show API key"}
                title={revealKey ? "Hide" : "Show"}
                onClick={() => setRevealKey((r) => !r)}
              >
                {revealKey ? <EyeOff size={15} /> : <Eye size={15} />}
              </button>
            </div>
          </label>
          <label className="field">
            <span className="field-name">Brave Search API key</span>
            <div className="field-with-action">
              <input
                className="field-input"
                type={revealBrave ? "text" : "password"}
                placeholder="Enables web_search — get one at brave.com/search/api"
                value={braveKey}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(e) => setBraveKey(e.target.value)}
              />
              <button
                type="button"
                className="field-action"
                aria-label={revealBrave ? "Hide Brave key" : "Show Brave key"}
                title={revealBrave ? "Hide" : "Show"}
                onClick={() => setRevealBrave((r) => !r)}
              >
                {revealBrave ? <EyeOff size={15} /> : <Eye size={15} />}
              </button>
            </div>
          </label>
        </div>
        <div className="settings-actions">
          <Button variant="primary" size="sm" onClick={save} disabled={saving || !loaded}>
            {saving ? "Saving…" : "Save connection"}
          </Button>
          {status && (
            <span className={`save-status ${status.ok ? "ok" : "err"}`}>
              {status.ok && <Check size={15} />}
              {status.msg}
            </span>
          )}
        </div>
        <p className="hint">
          Point the agent at a different Oxen endpoint, paste an API key, or add a Brave
          Search key to enable web search. Leave a field blank to fall back to your{" "}
          <code>OXEN_*</code> / <code>BRAVE_API_KEY</code> environment or <code>oxen</code> CLI
          login. Saving starts a new chat.
        </p>
      </section>

      <section className="settings-section">
        <div className="settings-label">Current session</div>
        <div className="meta">
          <Row label="model" value={session?.model ?? "—"} />
          <Row label="workspace" value={session?.workspace ?? "—"} title={session?.workspace} />
          <Row label="session" value={session ? session.session_id.slice(0, 8) : "—"} />
        </div>
        <p className="hint">
          The agent can read, write, and search files, run shell commands, and use git —
          scoped to the workspace above. Manage exactly which tools it may call on the{" "}
          <strong>Tools</strong> page.
        </p>
      </section>
    </div>
  );
}

/** Manage the cloud model catalog: list built-in + custom models, add a new one
 *  by id, remove custom ones, and pick the default. The default also swaps the
 *  current chat (continuing the conversation), matching the composer picker. */
function CloudModelsSettings() {
  const cloudModels = useStore((s) => s.cloudModels);
  const loadCloudModels = useStore((s) => s.loadCloudModels);
  const changeModel = useStore((s) => s.changeModel);

  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    loadCloudModels();
  }, [loadCloudModels]);

  async function add(e: FormEvent) {
    e.preventDefault();
    const trimmed = id.trim();
    if (!trimmed) return;
    setBusy(true);
    setError(null);
    try {
      await addCloudModel(trimmed, name.trim());
      await loadCloudModels();
      setId("");
      setName("");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function remove(modelId: string) {
    try {
      await removeCloudModel(modelId);
      await loadCloudModels();
    } catch (err) {
      setError(String(err));
    }
  }

  async function makeDefault(modelId: string) {
    try {
      await changeModel(modelId);
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <div className="settings-page">
      <section className="settings-section">
        <div className="settings-label">Models</div>
        <div className="model-list">
          {cloudModels.map((m) => (
            <div className={`model-item ${m.selected ? "selected" : ""}`} key={m.id}>
              <button
                className="model-default"
                title={m.selected ? "Current default" : "Make default"}
                aria-label={m.selected ? "Current default model" : "Make default model"}
                onClick={() => !m.selected && makeDefault(m.id)}
                disabled={m.selected}
              >
                <Star size={15} fill={m.selected ? "currentColor" : "none"} />
              </button>
              <div className="model-item-info">
                <span className="model-item-name">{m.name}</span>
                <span className="model-item-id">{m.id}</span>
              </div>
              {m.builtin ? (
                <span className="model-item-tag">built-in</span>
              ) : (
                <button
                  className="model-remove"
                  title="Remove model"
                  aria-label={`Remove ${m.name}`}
                  onClick={() => remove(m.id)}
                >
                  <Trash2 size={15} />
                </button>
              )}
            </div>
          ))}
        </div>

        <form className="model-add" onSubmit={add}>
          <input
            className="field-input"
            placeholder="Model id (e.g. claude-sonnet-4-6)"
            value={id}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            onChange={(e) => setId(e.target.value)}
          />
          <input
            className="field-input"
            placeholder="Display name (optional)"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          <Button variant="primary" size="sm" type="submit" disabled={busy || !id.trim()}>
            <Plus size={15} />
            Add
          </Button>
        </form>
        {error && <span className="save-status err">{error}</span>}
        <p className="hint">
          Add any model your Oxen endpoint serves by its id. Switch between models anytime
          from the picker beneath the chat box — the star marks the default for new chats.
        </p>
      </section>
    </div>
  );
}

/** Theme browsing/generation plus the light/dark mode toggle. */
function AppearanceSettings() {
  const mode = useStore((s) => s.mode);
  const toggleMode = useStore((s) => s.toggleMode);

  return (
    <div className="settings-page">
      <section className="settings-section">
        <div className="settings-label">Mode</div>
        <button className="mode-toggle" onClick={toggleMode}>
          {mode === "light" ? <Sun size={18} /> : <Moon size={18} />}
          <span>{mode === "light" ? "Light" : "Dark"} mode</span>
          <span className="nav-trail">toggle</span>
        </button>
      </section>

      <section className="settings-section">
        <div className="settings-label">Theme</div>
        <ThemesPanel />
      </section>
    </div>
  );
}

function Row({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div className="meta-row">
      <span className="meta-key">{label}</span>
      <span className="meta-val" title={title}>
        {value}
      </span>
    </div>
  );
}
