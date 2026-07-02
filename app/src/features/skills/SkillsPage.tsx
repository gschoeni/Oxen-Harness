// The Skills settings subpage: teach the agent reusable workflows without
// touching code. A skill is a SKILL.md — a one-line description (how the model
// decides when to use it) plus markdown instructions (what it does once
// loaded). Skills live globally (~/.oxen-harness/skills/) or inside the current
// project (.oxen-harness/skills/, shareable via git); only the name +
// description cost prompt tokens until the model actually loads one.

import { useEffect, useState } from "react";
import { ChevronRight, GraduationCap, Plus, Trash2 } from "lucide-react";
import { Button } from "../../components/ui";
import { deleteSkill, listSkills, saveSkill, setSkillEnabled } from "../../lib/ipc";
import type { SkillInfo, SkillScope } from "../../lib/types";
import { ToolSwitch } from "../tools/ToolSwitch";
import "../tools/tools.css";
import "./skills.css";

export function SkillsPage() {
  const [skills, setSkills] = useState<SkillInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Whether the "new skill" editor card is open.
  const [adding, setAdding] = useState(false);

  const load = () =>
    listSkills()
      .then(setSkills)
      .catch((e) => setError(String(e)));

  useEffect(() => {
    load();
  }, []);

  // Optimistically flip the toggle, then persist; reload on failure to resync.
  async function toggle(name: string, enabled: boolean) {
    setSkills((ss) => ss?.map((s) => (s.name === name ? { ...s, enabled } : s)) ?? ss);
    try {
      await setSkillEnabled(name, enabled);
    } catch (e) {
      setError(String(e));
      load();
    }
  }

  // Create or update a skill. A rename (or scope move) is save-then-delete —
  // save first, so a rejected edit can't lose the existing skill. Throws so
  // the editor can show the backend's message inline.
  async function save(draft: SkillDraft, prev?: SkillInfo) {
    await saveSkill(draft.scope, draft.name, draft.description, draft.instructions);
    if (prev && (prev.name !== draft.name || prev.scope !== draft.scope)) {
      await deleteSkill(prev.scope, prev.name);
    }
    await load();
  }

  async function remove(skill: SkillInfo) {
    await deleteSkill(skill.scope, skill.name);
    await load();
  }

  return (
    <div className="settings-page">
      <section className="settings-section">
        <div className="settings-label">
          Skills{skills && skills.length > 0 && ` · ${skills.length}`}
        </div>
        <p className="hint">
          A skill teaches the agent a reusable workflow — release notes in your house style, a
          review checklist, a deploy procedure. The model sees each skill's name and
          description, and loads the full instructions only when a request matches. Skills
          apply to <strong>new and resumed chats</strong>; project skills live in{" "}
          <code>.oxen-harness/skills/</code> so they can ship with the repo.
        </p>
        {error && <span className="save-status err">{error}</span>}

        <div className="tool-list">
          {skills === null ? (
            <p className="muted">Loading skills…</p>
          ) : (
            skills.map((s) => (
              <SkillRow key={`${s.scope}:${s.name}`} skill={s} onToggle={toggle} onSave={save} onDelete={remove} />
            ))
          )}

          {adding ? (
            <div className="tool-row tool-row-new">
              <div className="tool-editor-title">
                <GraduationCap size={14} className="tool-row-icon" />
                New skill
              </div>
              <SkillEditor
                onSave={async (draft) => {
                  await save(draft);
                  setAdding(false);
                }}
                onCancel={() => setAdding(false)}
              />
            </div>
          ) : (
            <button className="tool-add" onClick={() => setAdding(true)} disabled={skills === null}>
              <Plus size={16} />
              New skill
            </button>
          )}
        </div>
      </section>
    </div>
  );
}

/** One skill: expands into the same editor used to create it. */
function SkillRow({
  skill,
  onToggle,
  onSave,
  onDelete,
}: {
  skill: SkillInfo;
  onToggle: (name: string, enabled: boolean) => void;
  onSave: (draft: SkillDraft, prev: SkillInfo) => Promise<void>;
  onDelete: (skill: SkillInfo) => Promise<void>;
}) {
  const [open, setOpen] = useState(false);

  return (
    <div className={`tool-row ${skill.enabled ? "" : "disabled"}`}>
      <button className="tool-row-head" onClick={() => setOpen((v) => !v)}>
        <ChevronRight size={14} className={`tool-chevron ${open ? "open" : ""}`} />
        <GraduationCap size={14} className="tool-row-icon" />
        <span className="tool-row-name">{skill.name}</span>
        <span className={`skill-scope ${skill.scope}`}>{skill.scope}</span>
        {!open && <span className="tool-row-desc">{skill.description}</span>}
      </button>

      <ToolSwitch name={skill.name} enabled={skill.enabled} onToggle={onToggle} />

      {open && (
        <div className="tool-row-body">
          <SkillEditor
            initial={skill}
            onSave={async (draft) => {
              await onSave(draft, skill);
              setOpen(false);
            }}
            onDelete={() => onDelete(skill)}
          />
        </div>
      )}
    </div>
  );
}

/** What the editor produces — mirrors `saveSkill`'s arguments. */
interface SkillDraft {
  scope: SkillScope;
  name: string;
  description: string;
  instructions: string;
}

/** The create/edit form for a skill. `initial` present = editing (a rename or
 *  scope move re-homes the SKILL.md; a delete action appears). */
function SkillEditor({
  initial,
  onSave,
  onCancel,
  onDelete,
}: {
  initial?: SkillInfo;
  onSave: (draft: SkillDraft) => Promise<void>;
  onCancel?: () => void;
  onDelete?: () => Promise<void>;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [scope, setScope] = useState<SkillScope>(initial?.scope ?? "global");
  const [description, setDescription] = useState(initial?.description ?? "");
  const [instructions, setInstructions] = useState(initial?.instructions ?? "");

  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const canSave = name.trim() !== "" && description.trim() !== "" && instructions.trim() !== "";

  async function save() {
    setError(null);
    setSaving(true);
    try {
      await onSave({
        scope,
        name: name.trim(),
        description: description.trim(),
        instructions: instructions.trim(),
      });
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  async function reallyDelete() {
    if (!onDelete) return;
    setSaving(true);
    try {
      await onDelete();
    } catch (e) {
      setError(String(e));
      setSaving(false);
    }
  }

  return (
    <div className="tool-editor">
      <div className="tool-editor-grid">
        <label className="tool-field">
          <span className="field-name">Name</span>
          <input
            className="tool-input mono"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="release-notes"
            spellCheck={false}
          />
          <span className="tool-field-hint">Lowercase, hyphens — how the model refers to it.</span>
        </label>
        <label className="tool-field">
          <span className="field-name">Available in</span>
          <select
            className="tool-input"
            value={scope}
            onChange={(e) => setScope(e.target.value as SkillScope)}
            aria-label="Skill scope"
          >
            <option value="global">Every project (global)</option>
            <option value="project">This project only</option>
          </select>
          <span className="tool-field-hint">
            Project skills live in the repo, so your team gets them too.
          </span>
        </label>
      </div>

      <label className="tool-field">
        <span className="field-name">When should the model use it?</span>
        <input
          className="tool-input"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder="Writes release notes from the git log in our house style."
        />
        <span className="tool-field-hint">
          One line. The model reads this to decide when the skill applies.
        </span>
      </label>

      <label className="tool-field">
        <span className="field-name">Instructions</span>
        <textarea
          className="tool-desc-edit skill-instructions"
          rows={10}
          value={instructions}
          onChange={(e) => setInstructions(e.target.value)}
          placeholder={
            "Markdown the agent follows once the skill loads, e.g.\n\n" +
            "1. Run `git log --oneline` since the last tag.\n" +
            "2. Group changes into Added / Fixed / Changed.\n" +
            "3. Write one crisp line per change — no commit hashes."
          }
          spellCheck={false}
        />
      </label>

      {initial && (
        <span className="tool-field-hint">
          Stored at <code>{initial.dir}/SKILL.md</code> — supporting files placed beside it are
          available to the agent.
        </span>
      )}

      {error && <span className="save-status err">{error}</span>}

      <div className="tool-row-actions tool-editor-actions">
        <Button variant="primary" size="sm" disabled={!canSave || saving} onClick={save}>
          {saving ? "Saving…" : initial ? "Save changes" : "Add skill"}
        </Button>
        {onCancel && (
          <Button variant="ghost" size="sm" onClick={onCancel} disabled={saving}>
            Cancel
          </Button>
        )}
        {onDelete &&
          (confirmDelete ? (
            <Button variant="danger" size="sm" onClick={reallyDelete} disabled={saving}>
              Really delete?
            </Button>
          ) : (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setConfirmDelete(true)}
              title="Remove this skill and its files"
            >
              <Trash2 size={14} /> Delete skill
            </Button>
          ))}
      </div>
    </div>
  );
}
