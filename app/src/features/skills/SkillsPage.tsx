// The Skills settings subpage: teach the agent reusable workflows without
// touching code. A skill is a SKILL.md — a one-line description (how the model
// decides when to use it) plus markdown instructions (what it does once
// loaded). Skills live globally (~/.oxen-harness/skills/) or inside the current
// project (.oxen-harness/skills/, shareable via git); only the name +
// description cost prompt tokens until the model actually loads one.
//
// The page is a tiny three-view flow: the LIST of skills, a SHOW view that
// renders a skill's instructions as markdown, and an EDIT view with a
// write/preview markdown editor. Show and edit take over the whole subpage.

import { useEffect, useState } from "react";
import { ArrowLeft, ChevronDown, ChevronRight, GraduationCap, Pencil, Plus, Trash2 } from "lucide-react";
import { Button } from "../../components/ui";
import { Markdown } from "../../components/ui/Markdown";
import { deleteSkill, listSkills, saveSkill, setSkillEnabled } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { SkillInfo, SkillScope } from "../../lib/types";
import { ToolSwitch } from "../tools/ToolSwitch";
import "../tools/tools.css";
import "./skills.css";

/** Which of the three views is on screen. Show/edit reference a skill by name
 *  (scope disambiguates shadowed names). */
type View =
  | { kind: "list" }
  | { kind: "show"; name: string; scope: SkillScope }
  | { kind: "edit"; name: string; scope: SkillScope }
  | { kind: "new" };

export function SkillsPage() {
  const [skills, setSkills] = useState<SkillInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [view, setView] = useState<View>({ kind: "list" });

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
    setView({ kind: "show", name: draft.name, scope: draft.scope });
  }

  async function remove(skill: SkillInfo) {
    await deleteSkill(skill.scope, skill.name);
    await load();
    setView({ kind: "list" });
  }

  const find = (name: string, scope: SkillScope) =>
    skills?.find((s) => s.name === name && s.scope === scope) ?? null;

  // Show/edit views need their skill; if it vanished (deleted elsewhere, or a
  // reload dropped it) fall back to the list rather than a dead end.
  if (view.kind === "show" || view.kind === "edit") {
    const skill = find(view.name, view.scope);
    if (skills !== null && !skill) {
      setView({ kind: "list" });
    } else if (skill && view.kind === "show") {
      return (
        <SkillShow
          skill={skill}
          onBack={() => setView({ kind: "list" })}
          onEdit={() => setView({ kind: "edit", name: skill.name, scope: skill.scope })}
          onToggle={toggle}
        />
      );
    } else if (skill && view.kind === "edit") {
      return (
        <SkillEditor
          initial={skill}
          onSave={(draft) => save(draft, skill)}
          onCancel={() => setView({ kind: "show", name: skill.name, scope: skill.scope })}
          onDelete={() => remove(skill)}
        />
      );
    }
  }

  if (view.kind === "new") {
    return <SkillEditor onSave={(draft) => save(draft)} onCancel={() => setView({ kind: "list" })} />;
  }

  return (
    <div className="settings-page">
      <section className="settings-section">
        <div className="settings-label">
          Skills{skills && skills.length > 0 && ` · ${skills.length}`}
        </div>
        <SkillsExplainer />
        <p className="hint">
          A skill teaches the agent a reusable workflow — release notes in your house style, a
          review checklist, a deploy procedure. Skills apply to{" "}
          <strong>new and resumed chats</strong>; project skills live in{" "}
          <code>.oxen-harness/skills/</code> so they can ship with the repo.
        </p>
        {error && <span className="save-status err">{error}</span>}

        <div className="tool-list">
          {skills === null ? (
            <p className="muted">Loading skills…</p>
          ) : (
            skills.map((s) => (
              <SkillRow
                key={`${s.scope}:${s.name}`}
                skill={s}
                onOpen={() => setView({ kind: "show", name: s.name, scope: s.scope })}
                onToggle={toggle}
              />
            ))
          )}

          <button className="tool-add" onClick={() => setView({ kind: "new" })} disabled={skills === null}>
            <Plus size={16} />
            New skill
          </button>
        </div>
      </section>
    </div>
  );
}

/** The tools ↔ skills mental model, up front for first-time visitors: tools are
 *  abilities, skills are know-how riding on one `skill` tool. */
function SkillsExplainer() {
  const setPage = useStore((s) => s.setSettingsPage);
  return (
    <div className="skills-explainer">
      <div className="skills-explainer-col">
        <span className="skills-explainer-term">
          <button className="hint-link" onClick={() => setPage("tools")}>
            Tools
          </button>{" "}
          are what the agent can <em>do</em>
        </span>
        <span className="skills-explainer-def">
          Read files, run commands, search the web, call your APIs. Every tool is offered to
          the model on every request.
        </span>
      </div>
      <div className="skills-explainer-col">
        <span className="skills-explainer-term">
          Skills are what it <em>knows how to do</em>
        </span>
        <span className="skills-explainer-def">
          Instructions loaded on demand through one built-in <code>skill</code> tool: the model
          sees each skill's name + description, and pulls in the full instructions only when a
          request matches. Write the description like a trigger — "does X, use when Y".
        </span>
      </div>
    </div>
  );
}

// ---- list view ----------------------------------------------------------------

/** One skill in the list: click anywhere to read it; the switch toggles it. */
function SkillRow({
  skill,
  onOpen,
  onToggle,
}: {
  skill: SkillInfo;
  onOpen: () => void;
  onToggle: (name: string, enabled: boolean) => void;
}) {
  return (
    <div className={`tool-row ${skill.enabled ? "" : "disabled"}`}>
      <button className="tool-row-head" onClick={onOpen} aria-label={`Open skill ${skill.name}`}>
        <GraduationCap size={14} className="tool-row-icon" />
        <span className="tool-row-name">{skill.name}</span>
        <span className={`skill-scope ${skill.scope}`}>{skill.scope}</span>
        <span className="tool-row-desc">{skill.description}</span>
        <ChevronRight size={14} className="skill-row-go" />
      </button>
      <ToolSwitch name={skill.name} enabled={skill.enabled} onToggle={onToggle} />
    </div>
  );
}

// ---- show view ------------------------------------------------------------------

/** A full-subpage reading view: the skill's identity and its instructions
 *  rendered as markdown. */
function SkillShow({
  skill,
  onBack,
  onEdit,
  onToggle,
}: {
  skill: SkillInfo;
  onBack: () => void;
  onEdit: () => void;
  onToggle: (name: string, enabled: boolean) => void;
}) {
  return (
    <div className="settings-page skill-detail">
      <button className="skill-back" onClick={onBack}>
        <ArrowLeft size={15} />
        All skills
      </button>

      <header className="skill-detail-head">
        <GraduationCap size={20} className="skill-detail-icon" />
        <div className="skill-detail-titles">
          <h3 className="skill-detail-name">
            {skill.name}
            <span className={`skill-scope ${skill.scope}`}>{skill.scope}</span>
          </h3>
          <p className="skill-detail-desc">{skill.description}</p>
        </div>
        <div className="skill-detail-actions">
          <ToolSwitch name={skill.name} enabled={skill.enabled} onToggle={onToggle} />
          <Button variant="primary" size="sm" onClick={onEdit}>
            <Pencil size={14} /> Edit
          </Button>
        </div>
      </header>

      <article className="skill-md" aria-label="Skill instructions">
        <Markdown text={skill.instructions} />
      </article>

      <footer className="skill-detail-foot">
        Stored at <code>{skill.dir}/SKILL.md</code> — supporting files placed beside it are
        available to the agent.
      </footer>
    </div>
  );
}

// ---- edit view ------------------------------------------------------------------

/** What the editor produces — mirrors `saveSkill`'s arguments. */
interface SkillDraft {
  scope: SkillScope;
  name: string;
  description: string;
  instructions: string;
}

/** A full-subpage create/edit form. The instructions get a write/preview
 *  markdown editor; `initial` present = editing (a rename or scope move
 *  re-homes the SKILL.md; a delete action appears). */
function SkillEditor({
  initial,
  onSave,
  onCancel,
  onDelete,
}: {
  initial?: SkillInfo;
  onSave: (draft: SkillDraft) => Promise<void>;
  onCancel: () => void;
  onDelete?: () => Promise<void>;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [scope, setScope] = useState<SkillScope>(initial?.scope ?? "global");
  const [description, setDescription] = useState(initial?.description ?? "");
  const [instructions, setInstructions] = useState(initial?.instructions ?? "");
  const [previewing, setPreviewing] = useState(false);

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
    <div className="settings-page skill-detail">
      <button className="skill-back" onClick={onCancel}>
        <ArrowLeft size={15} />
        {initial ? initial.name : "All skills"}
      </button>

      <div className="settings-label">{initial ? `Edit ${initial.name}` : "New skill"}</div>

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
            <span className="tool-select">
              <select
                className="tool-input"
                value={scope}
                onChange={(e) => setScope(e.target.value as SkillScope)}
                aria-label="Skill scope"
              >
                <option value="global">Every project (global)</option>
                <option value="project">This project only</option>
              </select>
              <ChevronDown size={14} />
            </span>
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

        <div className="tool-field">
          <div className="tool-params-head">
            <span className="field-name">Instructions</span>
            <div className="tool-mode-toggle" role="group" aria-label="Instructions editor mode">
              <button className={previewing ? "" : "active"} onClick={() => setPreviewing(false)}>
                Write
              </button>
              <button className={previewing ? "active" : ""} onClick={() => setPreviewing(true)}>
                Preview
              </button>
            </div>
          </div>

          {previewing ? (
            <div className="skill-md skill-md-preview" aria-label="Instructions preview">
              {instructions.trim() ? (
                <Markdown text={instructions} />
              ) : (
                <p className="muted">Nothing to preview yet.</p>
              )}
            </div>
          ) : (
            <textarea
              className="tool-desc-edit skill-instructions"
              rows={16}
              value={instructions}
              onChange={(e) => setInstructions(e.target.value)}
              placeholder={
                "Markdown the agent follows once the skill loads, e.g.\n\n" +
                "1. Run `git log --oneline` since the last tag.\n" +
                "2. Group changes into Added / Fixed / Changed.\n" +
                "3. Write one crisp line per change — no commit hashes."
              }
              spellCheck={false}
              aria-label="Instructions markdown"
            />
          )}
        </div>

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
          <Button variant="ghost" size="sm" onClick={onCancel} disabled={saving}>
            Cancel
          </Button>
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
    </div>
  );
}
