import { useState } from "react";
import { FileText, Folder, FolderOpen, Image, Paperclip, Plus, X } from "lucide-react";
import { Button, IconButton, Modal } from "../../components/ui";
import { pickFolder, pickProjectContext, pickProjectParent, startProject } from "../../lib/ipc";
import type { Project } from "../../lib/types";

export function StartProjectModal({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: (project: Project) => Promise<void> | void;
}) {
  const [mode, setMode] = useState<"new" | "existing">("new");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [instructions, setInstructions] = useState("");
  const [directory, setDirectory] = useState("");
  const [contextPaths, setContextPaths] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  async function chooseDirectory() {
    const selected = mode === "new" ? await pickProjectParent() : await pickFolder();
    if (selected) setDirectory(selected);
  }

  async function addContext() {
    const selected = await pickProjectContext();
    setContextPaths((current) => [...new Set([...current, ...selected])]);
  }

  async function submit() {
    if (!name.trim() || !directory) return;
    setSaving(true);
    setError("");
    try {
      const project = await startProject({
        name: name.trim(),
        description: description.trim(),
        instructions: instructions.trim(),
        directory,
        createDirectory: mode === "new",
        contextPaths,
      });
      await onCreated(project);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal title="Start a project" onClose={onClose} wide>
      <div className="start-project-form">
        <div className="project-mode-grid" role="group" aria-label="Project folder type">
          <button className={mode === "new" ? "selected" : ""} onClick={() => { setMode("new"); setDirectory(""); }}>
            <Folder size={20} />
            <span><strong>Create a new folder</strong><small>Choose a location and we’ll create it from the project name.</small></span>
          </button>
          <button className={mode === "existing" ? "selected" : ""} onClick={() => { setMode("existing"); setDirectory(""); }}>
            <FolderOpen size={20} />
            <span><strong>Use existing folder</strong><small>Turn a codebase already on your computer into a project.</small></span>
          </button>
        </div>

        <label className="project-field">
          <span>Project name</span>
          <input autoFocus value={name} onChange={(event) => setName(event.target.value)} placeholder="Demo App" />
        </label>
        <label className="project-field">
          <span>Project goal</span>
          <textarea value={description} onChange={(event) => setDescription(event.target.value)} placeholder="What are you trying to achieve?" rows={3} />
        </label>
        <label className="project-field">
          <span>Project instructions <small>optional</small></span>
          <textarea aria-label="Project instructions" value={instructions} onChange={(event) => setInstructions(event.target.value)} placeholder="How should the agent approach work in this project?" rows={4} />
        </label>

        <div className="project-field">
          <span>{mode === "new" ? "Create inside" : "Project folder"}</span>
          <button className="project-folder-picker" onClick={() => void chooseDirectory()} aria-label="Choose project folder">
            <FolderOpen size={17} />
            <span>{directory || (mode === "new" ? "Choose a parent folder…" : "Choose the project folder…")}</span>
          </button>
          {mode === "new" && directory && name.trim() && (
            <small className="project-created-path">New folder: {directory}/{name.trim()}</small>
          )}
        </div>

        <div className="project-field">
          <span>Starting context <small>optional</small></span>
          <Button size="sm" onClick={() => void addContext()}><Plus size={15} /> Add context</Button>
          {contextPaths.length > 0 && (
            <div className="start-context-list">
              {contextPaths.map((path) => (
                <span key={path} className="start-context-chip" title={path}>
                  {contextIcon(path)} {basename(path)}
                  <IconButton aria-label={`Remove ${basename(path)}`} onClick={() => setContextPaths((items) => items.filter((item) => item !== path))}>
                    <X size={13} />
                  </IconButton>
                </span>
              ))}
            </div>
          )}
          <small className="project-field-hint">References are copied into .oxen-harness/context so they travel with the project.</small>
        </div>

        {error && <div className="project-form-error" role="alert">{error}</div>}
        <div className="project-form-actions">
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={!name.trim() || !directory || saving} onClick={() => void submit()}>
            {saving ? "Creating…" : "Create project"}
          </Button>
        </div>
      </div>
    </Modal>
  );
}

function basename(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

function contextIcon(path: string) {
  if (/\.(png|jpe?g|gif|webp|bmp|tiff|heic)$/i.test(path)) return <Image size={14} />;
  if (/\.pdf$/i.test(path)) return <Paperclip size={14} />;
  return <FileText size={14} />;
}
