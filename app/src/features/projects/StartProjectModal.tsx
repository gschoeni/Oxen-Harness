import { useEffect, useRef, useState } from "react";
import { Check, Folder, FolderOpen, Pin } from "lucide-react";
import { Button, Modal } from "../../components/ui";
import {
  getDefaultProjectLocation,
  pickFolder,
  pickProjectParent,
  setDefaultProjectLocation,
  startProject,
} from "../../lib/ipc";
import type { Project } from "../../lib/types";

export function StartProjectModal({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: (project: Project) => Promise<void> | void;
}) {
  const [mode, setMode] = useState<"new" | "existing">("new");
  const modeRef = useRef(mode);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [directory, setDirectory] = useState("");
  const [defaultDirectory, setDefaultDirectory] = useState("");
  const [saving, setSaving] = useState(false);
  const [savingDefault, setSavingDefault] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    void getDefaultProjectLocation()
      .then((saved) => {
        if (!active || !saved) return;
        setDefaultDirectory(saved);
        if (modeRef.current === "new") setDirectory((current) => current || saved);
      })
      .catch(() => {
        // A missing preference should never block creating a project.
      });
    return () => { active = false; };
  }, []);

  function selectMode(next: "new" | "existing") {
    modeRef.current = next;
    setMode(next);
    setDirectory(next === "new" ? defaultDirectory : "");
    setError("");
  }

  async function chooseDirectory() {
    const selected = mode === "new" ? await pickProjectParent() : await pickFolder();
    if (selected) setDirectory(selected);
  }

  async function makeDefault() {
    if (!directory || savingDefault) return;
    setSavingDefault(true);
    setError("");
    try {
      const canonical = await setDefaultProjectLocation(directory);
      setDefaultDirectory(canonical);
      setDirectory(canonical);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setSavingDefault(false);
    }
  }

  async function submit() {
    if (!name.trim() || !directory) return;
    setSaving(true);
    setError("");
    try {
      const project = await startProject({
        name: name.trim(),
        description: description.trim(),
        directory,
        createDirectory: mode === "new",
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
          <button className={mode === "new" ? "selected" : ""} onClick={() => selectMode("new")}>
            <Folder size={20} />
            <span><strong>Create a new folder</strong><small>Choose a location and we’ll create it from the project name.</small></span>
          </button>
          <button className={mode === "existing" ? "selected" : ""} onClick={() => selectMode("existing")}>
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

        <div className="project-field">
          <span>{mode === "new" ? "Create inside" : "Project folder"}</span>
          <button className="project-folder-picker" onClick={() => void chooseDirectory()} aria-label="Choose project folder">
            <FolderOpen size={17} />
            <span>{directory || (mode === "new" ? "Choose a parent folder…" : "Choose the project folder…")}</span>
          </button>
          {mode === "new" && (
            <div className="project-location-default">
              {directory && directory === defaultDirectory ? (
                <span><Check size={14} /> Default project location</span>
              ) : directory ? (
                <Button size="sm" disabled={savingDefault} onClick={() => void makeDefault()}>
                  <Pin size={14} /> {savingDefault ? "Saving…" : "Use as default"}
                </Button>
              ) : (
                <small>Choose a parent folder, then save it as the default for future projects.</small>
              )}
            </div>
          )}
          {mode === "new" && directory && name.trim() && (
            <small className="project-created-path">New folder: {directory}/{name.trim()}</small>
          )}
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
