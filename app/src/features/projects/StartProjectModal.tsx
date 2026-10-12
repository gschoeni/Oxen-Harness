import { useEffect, useRef, useState } from "react";
import { Check, FolderOpen, FolderPlus } from "lucide-react";
import { Button, Modal } from "../../components/ui";
import {
  getDefaultProjectLocation,
  pickFolder,
  pickProjectParent,
  setDefaultProjectLocation,
  startProject,
} from "../../lib/ipc";
import { PROMPT_INPUT_PROPS } from "../../lib/promptInput";
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
  // An existing folder names the project until the user types their own name.
  const nameEdited = useRef(false);

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
    if (!selected) return;
    setDirectory(selected);
    if (mode === "existing" && !nameEdited.current) setName(baseName(selected));
  }

  async function makeDefault() {
    if (!directory || savingDefault) return;
    const requestedDirectory = directory;
    setSavingDefault(true);
    setError("");
    try {
      const canonical = await setDefaultProjectLocation(requestedDirectory);
      setDefaultDirectory(canonical);
      setDirectory((current) => (
        modeRef.current === "new" && current === requestedDirectory ? canonical : current
      ));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setSavingDefault(false);
    }
  }

  async function submit() {
    if (!name.trim() || !directory || savingDefault) return;
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

  const trimmedName = name.trim();
  const isDefault = mode === "new" && !!directory && directory === defaultDirectory;

  return (
    <Modal title="Start a project" onClose={onClose} wide>
      <div className="start-project-form">
        <div className="project-field project-location">
          <div className="project-location-head">
            <span>Location</span>
            <div className="project-mode-switch" role="radiogroup" aria-label="Project folder type">
              <button role="radio" aria-checked={mode === "new"} className={mode === "new" ? "active" : ""} onClick={() => selectMode("new")}>
                New folder
              </button>
              <button role="radio" aria-checked={mode === "existing"} className={mode === "existing" ? "active" : ""} onClick={() => selectMode("existing")}>
                Existing folder
              </button>
            </div>
          </div>

          <button className="project-location-picker" onClick={() => void chooseDirectory()} aria-label="Choose project folder">
            {mode === "new" ? <FolderPlus size={16} /> : <FolderOpen size={16} />}
            {directory ? (
              <span className="project-location-path" title={mode === "new" ? joinPath(directory, trimmedName) : directory}>
                {mode === "new" ? (
                  <>
                    <span className="project-location-parent"><bdi>{withSlash(shortenHome(directory))}</bdi></span>
                    <span className={`project-location-leaf${trimmedName ? "" : " placeholder"}`}>
                      {trimmedName || "project-name"}
                    </span>
                  </>
                ) : (
                  <>
                    <span className="project-location-parent"><bdi>{withSlash(shortenHome(parentOf(directory)))}</bdi></span>
                    <span className="project-location-leaf">{baseName(directory)}</span>
                  </>
                )}
              </span>
            ) : (
              <span className="project-location-prompt">
                {mode === "new" ? "Choose where to create it…" : "Choose a folder on your computer…"}
              </span>
            )}
            <span className="project-location-change">{directory ? "Change" : "Browse"}</span>
          </button>

          {mode === "new" && directory && (
            <div className="project-field-hint project-location-default">
              {isDefault ? (
                <><Check size={12} /> Default location</>
              ) : (
                <button disabled={savingDefault} onClick={() => void makeDefault()}>
                  {savingDefault ? "Saving…" : "Make this the default location"}
                </button>
              )}
            </div>
          )}
        </div>

        <label className="project-field">
          <span>Project name</span>
          <input
            autoFocus
            value={name}
            onChange={(event) => { nameEdited.current = true; setName(event.target.value); }}
            placeholder="Demo App"
          />
        </label>
        <label className="project-field">
          <span>Goal <small>optional</small></span>
          <textarea {...PROMPT_INPUT_PROPS} value={description} onChange={(event) => setDescription(event.target.value)} placeholder="What are you trying to achieve?" rows={3} />
        </label>

        {error && <div className="project-form-error" role="alert">{error}</div>}
        <div className="project-form-actions">
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={!trimmedName || !directory || saving || savingDefault} onClick={() => void submit()}>
            {saving ? "Creating…" : "Create project"}
          </Button>
        </div>
      </div>
    </Modal>
  );
}

/** Display-only: `/Users/me/Code` reads as `~/Code`. Paths sent to the backend stay absolute. */
function shortenHome(path: string): string {
  return path.replace(/^\/(?:Users|home)\/[^/]+(?=\/|$)/, "~");
}

function withSlash(path: string): string {
  return path.endsWith("/") ? path : `${path}/`;
}

function joinPath(parent: string, leaf: string): string {
  return leaf ? `${withSlash(parent)}${leaf}` : parent;
}

function baseName(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || trimmed;
}

function parentOf(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  const cut = trimmed.lastIndexOf("/");
  return cut > 0 ? trimmed.slice(0, cut) : "/";
}
