import { FormEvent, useState } from "react";
import { ArrowLeft, FileImage, FileText, FolderOpen, MessageSquare, Paperclip, Pencil, Plus, Send, X } from "lucide-react";
import { Button, IconButton, Modal } from "../../components/ui";
import { addProjectContext, pickProjectContext, removeProjectContext, updateProject } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { Project, ProjectContext } from "../../lib/types";

export function ProjectHome({
  project,
  onBack,
  onProjectChanged,
}: {
  project: Project;
  onBack: () => void;
  onProjectChanged: (project: Project) => Promise<void> | void;
}) {
  const setProjectsOpen = useStore((state) => state.setProjectsOpen);
  const prepareProject = useStore((state) => state.prepareProject);
  const send = useStore((state) => state.send);
  const [prompt, setPrompt] = useState("");
  const [editing, setEditing] = useState(false);
  const [busyContext, setBusyContext] = useState(false);
  const [startingChat, setStartingChat] = useState(false);
  const [chatError, setChatError] = useState("");

  async function submit(event: FormEvent) {
    event.preventDefault();
    const text = prompt.trim();
    if (!text || startingChat) return;
    setStartingChat(true);
    setChatError("");
    try {
      await prepareProject(project.path);
      setProjectsOpen(false);
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
        <button className="project-breadcrumb" onClick={onBack}><ArrowLeft size={15} /> Projects</button>
        <div className="project-home-heading">
          <div>
            <h1>{project.name}</h1>
            <p>{project.description || "Add a goal so every chat starts with a shared destination."}</p>
            <span className="project-home-path"><FolderOpen size={13} /> {project.path}</span>
          </div>
          <Button onClick={() => setEditing(true)} aria-label="Edit project"><Pencil size={15} /> Edit project</Button>
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
              <span><MessageSquare size={15} /> A fresh chat with this project’s context</span>
              <IconButton type="submit" className="project-send" aria-label="Send project prompt" disabled={!prompt.trim() || startingChat}>
                <Send size={17} />
              </IconButton>
            </div>
          </form>
          {chatError && <div className="project-home-error" role="alert">Could not start this chat: {chatError}</div>}
          <div className="project-home-empty">
            <MessageSquare size={28} />
            <p>Start with a task and the agent will pick up the project goal, instructions, and references automatically.</p>
          </div>
        </section>

        <aside className="project-context-panel">
          <section className="project-context-card">
            <div className="project-context-card-header">
              <div><h2>Instructions</h2><p>Durable guidance for every new chat.</p></div>
              <IconButton aria-label="Edit project instructions" onClick={() => setEditing(true)}><Pencil size={16} /></IconButton>
            </div>
            <div className={project.instructions ? "project-instructions" : "project-instructions empty"}>
              {project.instructions || "No special instructions yet."}
            </div>
          </section>

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

      {editing && (
        <EditProjectModal
          project={project}
          onClose={() => setEditing(false)}
          onSave={async (name, description, instructions) => {
            await onProjectChanged(await updateProject(project.path, name, description, instructions));
            setEditing(false);
          }}
        />
      )}
    </main>
  );
}

function EditProjectModal({
  project,
  onClose,
  onSave,
}: {
  project: Project;
  onClose: () => void;
  onSave: (name: string, description: string, instructions: string) => Promise<void>;
}) {
  const [name, setName] = useState(project.name);
  const [description, setDescription] = useState(project.description);
  const [instructions, setInstructions] = useState(project.instructions);
  const [saving, setSaving] = useState(false);

  return (
    <Modal title="Edit project" onClose={onClose} wide>
      <div className="start-project-form">
        <label className="project-field"><span>Project name</span><input value={name} onChange={(event) => setName(event.target.value)} /></label>
        <label className="project-field"><span>Project goal</span><textarea rows={3} value={description} onChange={(event) => setDescription(event.target.value)} /></label>
        <label className="project-field"><span>Project instructions</span><textarea rows={7} value={instructions} onChange={(event) => setInstructions(event.target.value)} /></label>
        <small className="project-field-hint">The next prompt starts a fresh chat with this updated guidance.</small>
        <div className="project-form-actions">
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={!name.trim() || saving} onClick={async () => {
            setSaving(true);
            try { await onSave(name.trim(), description.trim(), instructions.trim()); } finally { setSaving(false); }
          }}>{saving ? "Saving…" : "Save changes"}</Button>
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
