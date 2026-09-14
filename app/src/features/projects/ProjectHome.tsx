import { FormEvent, useEffect, useState } from "react";
import { ArrowLeft, Check, ExternalLink, FileImage, FileText, FolderOpen, GitBranch, MessageSquare, Paperclip, Pencil, Plus, Send, Trash2, X } from "lucide-react";
import { Button, IconButton, Modal } from "../../components/ui";
import { addProjectContext, getConnection, openExternal, pickProjectContext, removeProjectContext, updateProject } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { Project, ProjectContext, StartupModelChoice } from "../../lib/types";
import { ModelPicker } from "../chat/ModelPicker";
import { ProjectTrail } from "../ledger/ProjectTrail";
import { ProjectMediaCard } from "../media/ProjectMediaCard";
import { useBoard } from "../ledger/useBoard";
import { RemoveProjectModal } from "./RemoveProjectModal";
import "./projects.css";

export function ProjectHome({
  project,
  onBack,
  onProjectChanged,
}: {
  project: Project;
  onBack: () => void;
  onProjectChanged: (project: Project) => Promise<void> | void;
}) {
  const setHomeOpen = useStore((state) => state.setHomeOpen);
  const prepareProject = useStore((state) => state.prepareProject);
  const send = useStore((state) => state.send);
  const [prompt, setPrompt] = useState("");
  const [name, setName] = useState(project.name);
  const [goal, setGoal] = useState(project.description);
  const [savingDetails, setSavingDetails] = useState(false);
  const [editingInstructions, setEditingInstructions] = useState(false);
  const [busyContext, setBusyContext] = useState(false);
  const [startingChat, setStartingChat] = useState(false);
  const [chatError, setChatError] = useState("");
  const [startupModel, setStartupModel] = useState<StartupModelChoice | null>(null);
  const removeProject = useStore((state) => state.removeProject);
  const [pendingDelete, setPendingDelete] = useState(false);
  const cleanName = name.trim();
  const cleanGoal = goal.trim();
  const detailsChanged = name !== project.name || goal !== project.description;
  // Whether this project has anything to show on its trail (open, settled, or
  // lost threads) — with history, the getting-started hint yields to it.
  const board = useBoard();
  const hasTrail =
    !!board &&
    (board.trains.some((t) => t.workspace === project.path) ||
      board.settled.some((t) => t.entry.workspace === project.path) ||
      board.lost.some((t) => t.entry.workspace === project.path));

  async function saveDetails() {
    if (!cleanName || !detailsChanged || savingDetails) return;
    setSavingDetails(true);
    try {
      await onProjectChanged(
        await updateProject(project.path, cleanName, cleanGoal, project.instructions, project.remote_repo),
      );
      setName(cleanName);
      setGoal(cleanGoal);
    } finally {
      setSavingDetails(false);
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    const text = prompt.trim();
    if (!text || startingChat) return;
    setStartingChat(true);
    setChatError("");
    try {
      await prepareProject(project.path, startupModel ?? undefined);
      setHomeOpen(false);
      send(text);
    } catch (reason) {
      setChatError(String(reason));
    } finally {
      setStartingChat(false);
    }
  }

  async function addContext() {
    const paths = await pickProjectContext();
    if (!paths.length) return;
    setBusyContext(true);
    try {
      await onProjectChanged(await addProjectContext(project.path, paths));
    } finally {
      setBusyContext(false);
    }
  }

  async function removeContext(context: ProjectContext) {
    setBusyContext(true);
    try {
      await onProjectChanged(await removeProjectContext(project.path, context.path));
    } finally {
      setBusyContext(false);
    }
  }

  return (
    <main className="project-home">
      <header className="project-home-header">
        <button className="project-breadcrumb" onClick={onBack}><ArrowLeft size={13} /> Home</button>
        {/* The quiet way out, same manners as the cards: corner trash, always
            behind a confirm, never louder than the work. */}
        <button
          className="project-home-delete"
          title="Remove project"
          aria-label={`Remove project: ${cleanName || project.name}`}
          onClick={() => setPendingDelete(true)}
        >
          <Trash2 size={15} />
        </button>
        <div className="project-home-heading">
          <div className="project-home-identity">
            <h1 aria-label={cleanName || "Untitled project"}>
              <input
                aria-label="Project name"
                value={name}
                onChange={(event) => setName(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    void saveDetails();
                  }
                }}
                spellCheck={false}
              />
            </h1>
            <textarea
              className="project-home-goal"
              aria-label="Project goal"
              value={goal}
              onChange={(event) => setGoal(event.target.value)}
              placeholder="Add a goal so every chat starts with a shared destination."
              rows={2}
            />
            <div className="project-home-meta">
              <span className="project-home-path"><FolderOpen size={13} /> {project.path}</span>
              {detailsChanged && (
                <Button
                  variant="primary"
                  disabled={!cleanName || savingDetails}
                  onClick={() => void saveDetails()}
                >
                  {savingDetails ? "Saving…" : "Save project details"}
                </Button>
              )}
            </div>
          </div>
        </div>
      </header>

      <div className="project-home-grid">
        <section className="project-home-main">
          <form className="project-composer" onSubmit={(event) => void submit(event)}>
            <textarea
              aria-label="Ask about this project"
              value={prompt}
              onChange={(event) => setPrompt(event.target.value)}
              placeholder="What should we work on?"
              rows={4}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.shiftKey) {
                  event.preventDefault();
                  event.currentTarget.form?.requestSubmit();
                }
              }}
            />
            <div className="project-composer-footer">
              <div className="project-composer-options">
                <ModelPicker
                  disabled={startingChat}
                  startupChoice={startupModel}
                  onStartupChoice={setStartupModel}
                />
                <span><MessageSquare size={15} /> A fresh chat with this project’s context</span>
              </div>
              <IconButton type="submit" className="project-send" aria-label="Send project prompt" disabled={!prompt.trim() || startingChat}>
                <Send size={17} />
              </IconButton>
            </div>
          </form>
          {chatError && <div className="project-home-error" role="alert">Could not start this chat: {chatError}</div>}
          {/* The project's full trail — every open thread, no cap. The home
              board's "…and N more on this trail" lands here. A project with
              history doesn't need the getting-started hint anymore. */}
          {hasTrail ? (
            <ProjectTrail workspace={project.path} />
          ) : (
            <div className="project-home-empty">
              <MessageSquare size={28} />
              <p>Start with a task and the agent will pick up the project goal, instructions, and references automatically.</p>
            </div>
          )}
          <ProjectMediaCard path={project.path} />
        </section>

        <aside className="project-context-panel">
          <section className="project-context-card">
            <div className="project-context-card-header">
              <div><h2>Instructions</h2><p>Durable guidance for every new chat.</p></div>
              <IconButton aria-label="Edit project instructions" onClick={() => setEditingInstructions(true)}><Pencil size={16} /></IconButton>
            </div>
            <div className={project.instructions ? "project-instructions" : "project-instructions empty"}>
              {project.instructions || "No special instructions yet."}
            </div>
          </section>

          <RepositoryCard
            project={project}
            onSave={async (remoteRepo) => {
              await onProjectChanged(
                await updateProject(project.path, project.name, project.description, project.instructions, remoteRepo),
              );
            }}
          />

          <section className="project-context-card">
            <div className="project-context-card-header">
              <div><h2>Context</h2><p>References the agent can use across chats.</p></div>
              <IconButton disabled={busyContext} aria-label="Add project context" onClick={() => void addContext()}><Plus size={17} /></IconButton>
            </div>
            {project.context.length ? (
              <div className="project-context-files">
                {project.context.map((context) => (
                  <div className="project-context-file" key={context.path}>
                    <span className="project-context-file-icon">{fileIcon(context)}</span>
                    <span><strong>{context.name}</strong><small>{context.kind.toUpperCase()} · {formatBytes(context.size_bytes)}</small></span>
                    <IconButton disabled={busyContext} aria-label={`Remove ${context.name}`} onClick={() => void removeContext(context)}><X size={14} /></IconButton>
                  </div>
                ))}
              </div>
            ) : (
              <button className="project-context-empty" disabled={busyContext} onClick={() => void addContext()}>
                <Paperclip size={20} /><span>Add documents, images, or other text references.</span>
              </button>
            )}
          </section>
        </aside>
      </div>

      {pendingDelete && (
        <RemoveProjectModal
          name={cleanName || project.name}
          onCancel={() => setPendingDelete(false)}
          onConfirm={async () => {
            await removeProject(project.path);
            setPendingDelete(false);
            // The project is gone — there is no page left to stand on.
            onBack();
          }}
        />
      )}

      {editingInstructions && (
        <EditInstructionsModal
          instructions={project.instructions}
          onClose={() => setEditingInstructions(false)}
          onSave={async (instructions) => {
            await onProjectChanged(
              await updateProject(project.path, project.name, project.description, instructions, project.remote_repo),
            );
            setEditingInstructions(false);
          }}
        />
      )}
    </main>
  );
}

/** The project's remote Oxen repository: `namespace/name` on the hub. The
 *  same setting the agent's `create_repository` tool fills in and
 *  `oxen-harness project set-repo` edits — one field, saved on Enter or
 *  the check, cleared by emptying it. */
function RepositoryCard({
  project,
  onSave,
}: {
  project: Project;
  onSave: (remoteRepo: string | null) => Promise<void>;
}) {
  const current = project.remote_repo ?? "";
  const [value, setValue] = useState(current);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const host = useHubHost();
  const clean = value.trim().replace(/^\/+|\/+$/g, "");
  const changed = clean !== current;
  const wellFormed = clean === "" || /^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/.test(clean);
  const url = current ? repoUrl(host, current) : "";

  async function save() {
    if (!changed || !wellFormed || saving) return;
    setSaving(true);
    setError("");
    try {
      await onSave(clean === "" ? null : clean);
      setValue(clean);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="project-context-card project-repo-card">
      <div className="project-context-card-header">
        <div><h2>Repository</h2><p>Where this project lives on the hub. Generated media keeps copies there.</p></div>
        {url && (
          <IconButton aria-label="Open repository on the hub" title={url} onClick={() => void openExternal(url)}>
            <ExternalLink size={16} />
          </IconButton>
        )}
      </div>
      <div className="project-repo-row">
        <GitBranch size={14} className="project-repo-icon" />
        <input
          aria-label="Remote Oxen repository"
          value={value}
          placeholder="namespace/name"
          spellCheck={false}
          onChange={(event) => setValue(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              void save();
            }
          }}
        />
        {changed && (
          <IconButton
            aria-label="Save repository"
            disabled={!wellFormed || saving}
            onClick={() => void save()}
          >
            <Check size={15} />
          </IconButton>
        )}
      </div>
      {!wellFormed ? (
        <small className="project-field-hint project-repo-error">Use the form <code>namespace/name</code>, e.g. <code>ox/my-app</code>.</small>
      ) : error ? (
        <small className="project-field-hint project-repo-error" role="alert">{error}</small>
      ) : current ? null : (
        <small className="project-field-hint">None yet — set one here, or ask the agent to create a repository for this project.</small>
      )}
    </section>
  );
}

/** The hub host the app is connected to (Settings → Connection), so the
 *  repository link points at the same hub the agent's tool created it on. */
function useHubHost(): string {
  const [host, setHost] = useState("hub.oxen.ai");
  useEffect(() => {
    let live = true;
    getConnection()
      .then((c) => {
        if (live && c.host) setHost(c.host);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);
  return host;
}

/** The repository's page on `host` (a bare host or a full origin). */
function repoUrl(host: string, remote: string): string {
  const origin = host.includes("://") ? host.replace(/\/+$/, "") : `https://${host}`;
  return `${origin}/${remote}`;
}

function EditInstructionsModal({
  instructions: initialInstructions,
  onClose,
  onSave,
}: {
  instructions: string;
  onClose: () => void;
  onSave: (instructions: string) => Promise<void>;
}) {
  const [instructions, setInstructions] = useState(initialInstructions);
  const [saving, setSaving] = useState(false);

  return (
    <Modal title="Edit instructions" onClose={onClose}>
      <div className="start-project-form">
        <label className="project-field"><span>Project instructions</span><textarea rows={7} value={instructions} onChange={(event) => setInstructions(event.target.value)} /></label>
        <small className="project-field-hint">These instructions are included in every new chat for this project.</small>
        <div className="project-form-actions">
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={saving} onClick={async () => {
            setSaving(true);
            try { await onSave(instructions.trim()); } finally { setSaving(false); }
          }}>{saving ? "Saving…" : "Save instructions"}</Button>
        </div>
      </div>
    </Modal>
  );
}

function fileIcon(context: ProjectContext) {
  if (context.kind === "image") return <FileImage size={17} />;
  if (context.kind === "pdf") return <Paperclip size={17} />;
  return <FileText size={17} />;
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
