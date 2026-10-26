// The Skills settings subpage: teach the agent reusable workflows without
// touching code. A skill is a SKILL.md — a one-line description (how the model
// decides when to use it) plus markdown instructions (what it does once
// loaded). Skills live globally (~/.oxen-harness/skills/) or inside the current
// project (.oxen-harness/skills/, shareable via git); only the name +
// description cost prompt tokens until the model actually loads one.
//
// The page is a small flow: the LIST of skills, a SHOW view that renders a
// skill's instructions as markdown, a NEW view that names the skill and picks
// where it lives (creating it on disk as a draft), an EDIT view with a
// write/preview markdown editor that saves as you type, and an IMPORT view.
// Everything but the list takes over the whole subpage.
//
// Skills travel as files: export writes a `<name>.zip` holding the skill's
// folder (the layout claude.ai's "Upload skill" takes, and what Codex and
// Claude Code keep on disk), and import takes that zip, a `.skill`, or a
// lone SKILL.md.

import { useEffect, useRef, useState, type FormEvent } from "react";
import {
  ArrowLeft,
  Check,
  ChevronRight,
  Download,
  GraduationCap,
  Pencil,
  Plus,
  Trash2,
  Upload,
} from "lucide-react";
import { Button } from "../../components/ui";
import { Select } from "../../components/ui/Select";
import {
  createSkill,
  deleteSkill,
  exportSkill,
  importSkill,
  listSkills,
  listTools,
  pickSkillExportPath,
  pickSkillFile,
  renameSkill,
  saveSkill,
  setSkillEnabled,
} from "../../lib/ipc";
import { useActiveProject } from "../../lib/store";
import type { SkillInfo, SkillScope, ToolInfo } from "../../lib/types";
import { InstructionsEditor, SkillMarkdown } from "./InstructionsEditor";
import { ToolSwitch } from "../tools/ToolSwitch";
import { TeachingNav } from "../settings/TeachingNav";
import { useAutosave } from "./useAutosave";
import "../tools/tools.css";
import "./skills.css";

/** Which view is on screen. Show/edit reference a skill by name (scope
 *  disambiguates shadowed names). */
type View =
  | { kind: "list" }
  | { kind: "show"; name: string; scope: SkillScope }
  | { kind: "edit"; name: string; scope: SkillScope }
  | { kind: "new" }
  | { kind: "import" };

export function SkillsPage() {
  const [skills, setSkills] = useState<SkillInfo[] | null>(null);
  // The tool vocabulary, for autocomplete, the palette, and reference
  // highlighting. Skills direct the agent to tools by naming them in backticks.
  const [tools, setTools] = useState<ToolInfo[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [view, setView] = useState<View>({ kind: "list" });
  const toolNames = tools.map((t) => t.name);

  const load = () =>
    listSkills()
      .then(setSkills)
      .catch((e) => setError(String(e)));

  useEffect(() => {
    load();
    listTools()
      .then(setTools)
      .catch(() => {}); // tool hints are progressive enhancement — no error UI
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

  // Reload the list and switch views in one render, so a view never points
  // at a skill the list doesn't have yet (which would bounce to the list).
  async function reloadThen(next: View) {
    const fresh = await listSkills();
    setSkills(fresh);
    setView(next);
  }

  // The functions below throw so the view can show the backend's message.
  async function create(id: SkillIdentity) {
    await createSkill(id.scope, id.name);
    await reloadThen({ kind: "edit", ...id });
  }


  async function importFrom(scope: SkillScope, path: string) {
    const name = await importSkill(scope, path);
    await reloadThen({ kind: "show", name, scope });
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
          toolNames={toolNames}
          onBack={() => setView({ kind: "list" })}
          onEdit={() => setView({ kind: "edit", name: skill.name, scope: skill.scope })}
          onToggle={toggle}
        />
      );
    } else if (skill && view.kind === "edit") {
      return (
        <SkillEditor
          skill={skill}
          tools={tools}
          onRename={(to) => renameSkill(skill.scope, skill.name, to.scope, to.name)}
          onMoved={(to) => reloadThen({ kind: "edit", ...to })}
          onDone={() => reloadThen({ kind: "show", name: skill.name, scope: skill.scope })}
          onDelete={() => remove(skill)}
        />
      );
    }
  }

  if (view.kind === "new") {
    return <SkillCreate onCreate={create} onCancel={() => setView({ kind: "list" })} />;
  }

  if (view.kind === "import") {
    return <SkillImport onImport={importFrom} onCancel={() => setView({ kind: "list" })} />;
  }

  return (
    <div className="settings-page">
      <TeachingNav current="skills" />
      <section className="settings-section">
        <div className="settings-label">
          Skills{skills && skills.length > 0 && ` · ${skills.length}`}
        </div>
        <p className="hint">
          A skill teaches the agent a reusable workflow — release notes in your house style, a
          review checklist, a deploy procedure. The model sees only each skill's name and
          description until it pulls the instructions in, so write the description like a
          trigger: "does X, use when Y". Skills apply to <strong>new and resumed chats</strong>;
          project skills live in <code>.oxen-harness/skills/</code> so they can ship with the
          repo.
        </p>
        {error && <span className="save-status err">{error}</span>}

        {skills === null ? (
          <p className="muted">Loading skills…</p>
        ) : (
          <SkillGroups
            skills={skills}
            onOpen={(s) => setView({ kind: "show", name: s.name, scope: s.scope })}
            onToggle={toggle}
          />
        )}

        <div className="tool-list">
          <Button variant="outline" className="tool-add" onClick={() => setView({ kind: "new" })} disabled={skills === null}>
            <Plus size={15} />
            New skill
          </Button>
          <Button variant="outline" className="tool-add" onClick={() => setView({ kind: "import" })} disabled={skills === null}>
            <Upload size={15} />
            Import skill
          </Button>
        </div>
      </section>
    </div>
  );
}

/** The skill list, grouped by where each skill lives so the context is
 *  unmissable: this project's skills first (named), then the global set. */
function SkillGroups({
  skills,
  onOpen,
  onToggle,
}: {
  skills: SkillInfo[];
  onOpen: (skill: SkillInfo) => void;
  onToggle: (name: string, enabled: boolean) => void;
}) {
  const project = useActiveProject();
  const groups = [
    {
      key: "project",
      label: project ? `This project · ${project.name}` : "This project",
      title: project?.path,
      skills: skills.filter((s) => s.scope === "project"),
    },
    {
      key: "global",
      label: "Global · every project",
      title: undefined as string | undefined,
      skills: skills.filter((s) => s.scope === "global"),
    },
  ].filter((g) => g.skills.length > 0);

  return (
    <>
      {groups.map((g) => (
        <div className="skills-group" key={g.key}>
          <div className="skills-group-label" title={g.title}>
            {g.label}
          </div>
          <div className="tool-list">
            {g.skills.map((s) => (
              <SkillRow
                key={`${s.scope}:${s.name}`}
                skill={s}
                onOpen={() => onOpen(s)}
                onToggle={onToggle}
              />
            ))}
          </div>
        </div>
      ))}
    </>
  );
}

/** The tools ↔ skills mental model, up front for first-time visitors: tools are
 *  abilities, skills are know-how riding on one `skill` tool. */

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
        <GraduationCap size={15} className="tool-row-icon" />
        <span className="tool-row-name">{skill.name}</span>
        {skill.draft && <span className="skill-scope draft">draft</span>}
        <span className="tool-row-desc">{skill.description}</span>
        <ChevronRight size={15} className="skill-row-go" />
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
  toolNames,
  onBack,
  onEdit,
  onToggle,
}: {
  skill: SkillInfo;
  toolNames: string[];
  onBack: () => void;
  onEdit: () => void;
  onToggle: (name: string, enabled: boolean) => void;
}) {
  const project = useActiveProject();
  const [exported, setExported] = useState<{ path?: string; error?: string } | null>(null);
  const scopeLabel =
    skill.scope === "project" && project ? `project · ${project.name}` : skill.scope;

  async function exportZip() {
    setExported(null);
    try {
      const path = await pickSkillExportPath(skill.name);
      if (!path) return;
      await exportSkill(skill.scope, skill.name, path);
      setExported({ path });
    } catch (e) {
      setExported({ error: String(e) });
    }
  }

  return (
    <div className="settings-page skill-detail">
      <Button size="sm" variant="ghost" className="skill-back" onClick={onBack}>
        <ArrowLeft size={15} />
        All skills
      </Button>

      <header className="skill-detail-head">
        <GraduationCap size={20} className="skill-detail-icon" />
        <div className="skill-detail-titles">
          <h3 className="skill-detail-name">
            {skill.name}
            <span className={`skill-scope ${skill.scope}`} title={skill.dir}>
              {scopeLabel}
            </span>
          </h3>
          <p className="skill-detail-desc">{skill.description}</p>
          {skill.draft && <DraftNotice />}
        </div>
        <div className="skill-detail-actions">
          <ToolSwitch name={skill.name} enabled={skill.enabled} onToggle={onToggle} />
          <Button
            variant="outline"
            size="sm"
            onClick={exportZip}
            title="Save as a .zip that Claude, Codex, Claude Code, and this app can import"
          >
            <Download size={15} /> Export
          </Button>
          <Button variant="primary" size="sm" onClick={onEdit}>
            <Pencil size={15} /> Edit
          </Button>
        </div>
      </header>

      {exported?.path && (
        <span className="save-status ok">
          Exported to <code>{exported.path}</code>
        </span>
      )}
      {exported?.error && <span className="save-status err">{exported.error}</span>}

      <article className="skill-md" aria-label="Skill instructions">
        {skill.instructions ? (
          <SkillMarkdown text={skill.instructions} toolNames={toolNames} />
        ) : (
          <p className="muted">No instructions yet.</p>
        )}
      </article>

      <footer className="skill-detail-foot">
        Stored at <code>{skill.dir}/SKILL.md</code> — supporting files placed beside it are
        available to the agent.
      </footer>
    </div>
  );
}

// ---- new + edit views --------------------------------------------------------

/** Where a skill lives on disk: its name and scope. */
interface SkillIdentity {
  name: string;
  scope: SkillScope;
}

function DraftNotice() {
  return (
    <p className="skill-draft-notice">
      Draft — the model won't see this skill until it has a description and instructions.
    </p>
  );
}

/** The name + "available in" pair, shared by the new-skill screen and the
 *  editor's rename/move form. */
function IdentityFields({
  value,
  onChange,
}: {
  value: SkillIdentity;
  onChange: (value: SkillIdentity) => void;
}) {
  return (
    <div className="tool-editor-grid">
      <label className="tool-field">
        <span className="field-name">Name</span>
        <input
          className="tool-input mono"
          value={value.name}
          onChange={(e) => onChange({ ...value, name: e.target.value })}
          placeholder="release-notes"
          spellCheck={false}
          autoFocus
        />
        <span className="tool-field-hint">Lowercase, hyphens — how the model refers to it.</span>
      </label>
      <ScopeField value={value.scope} onChange={(scope) => onChange({ ...value, scope })} />
    </div>
  );
}

/** "Available in": every project, or only the active one. */
function ScopeField({
  value,
  onChange,
}: {
  value: SkillScope;
  onChange: (value: SkillScope) => void;
}) {
  const project = useActiveProject();
  return (
    <label className="tool-field">
      <span className="field-name">Available in</span>
      <Select
        label="Skill scope"
        value={value}
        options={[
          { value: "global", label: "Every project (global)" },
          {
            value: "project",
            label: project ? `This project only — ${project.name}` : "This project only",
          },
        ]}
        onValueChange={(scope) => onChange(scope as SkillScope)}
      />
      <span className="tool-field-hint" title={project?.path}>
        {value === "project" && project
          ? `Saved into ${project.name}'s repo (.oxen-harness/skills/), so your team gets it too.`
          : "Project skills live in the repo, so your team gets them too."}
      </span>
    </label>
  );
}

/** Choose where an imported skill goes, then pick the file. */
function SkillImport({
  onImport,
  onCancel,
}: {
  onImport: (scope: SkillScope, path: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [scope, setScope] = useState<SkillScope>("global");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function choose() {
    setError(null);
    try {
      const path = await pickSkillFile();
      if (!path) return;
      setBusy(true);
      await onImport(scope, path);
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  return (
    <div className="settings-page skill-detail">
      <Button size="sm" variant="ghost" className="skill-back" onClick={onCancel}>
        <ArrowLeft size={15} />
        All skills
      </Button>

      <div className="settings-label">Import skill</div>
      <p className="hint">
        Takes a skill shared as a <code>.zip</code> or <code>.skill</code> file (the format
        Claude uses for uploading skills, and what <strong>Export</strong> here produces), or a
        single <code>SKILL.md</code>. Skills from Codex or Claude Code are folders — zip the folder
        to bring it in. Supporting files come along.
      </p>

      <div className="tool-editor">
        <div className="tool-editor-grid">
          <ScopeField value={scope} onChange={setScope} />
        </div>
        {error && <span className="save-status err">{error}</span>}
        <div className="tool-row-actions tool-editor-actions">
          <Button variant="primary" size="sm" onClick={choose} disabled={busy}>
            <Upload size={15} />
            {busy ? "Importing…" : "Choose file…"}
          </Button>
          <Button variant="ghost" size="sm" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
        </div>
      </div>
    </div>
  );
}

/** Name the skill and choose where it lives. Creating it writes an empty
 *  draft to disk, so everything typed afterward has a home to save into. */
function SkillCreate({
  onCreate,
  onCancel,
}: {
  onCreate: (id: SkillIdentity) => Promise<void>;
  onCancel: () => void;
}) {
  const [id, setId] = useState<SkillIdentity>({ name: "", scope: "global" });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setError(null);
    setBusy(true);
    try {
      await onCreate({ ...id, name: id.name.trim() });
    } catch (err) {
      setError(String(err));
      setBusy(false);
    }
  }

  return (
    <div className="settings-page skill-detail">
      <Button size="sm" variant="ghost" className="skill-back" onClick={onCancel}>
        <ArrowLeft size={15} />
        All skills
      </Button>

      <div className="settings-label">New skill</div>
      <p className="hint">
        Name it and choose where it's available. Next you'll write what it does — everything
        saves as you type, and you can rename or move it later.
      </p>

      <form className="tool-editor" onSubmit={submit}>
        <IdentityFields value={id} onChange={setId} />
        {error && <span className="save-status err">{error}</span>}
        <div className="tool-row-actions tool-editor-actions">
          <Button type="submit" variant="primary" size="sm" disabled={!id.name.trim() || busy}>
            {busy ? "Creating…" : "Create skill"}
          </Button>
          <Button type="button" variant="ghost" size="sm" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
        </div>
      </form>
    </div>
  );
}

/** A full-subpage editor for an existing skill. The description and
 *  instructions save as you type (and when focus leaves them); the name and
 *  scope change through an explicit rename/move, which re-homes the folder. */
function SkillEditor({
  skill,
  tools,
  onRename,
  onMoved,
  onDone,
  onDelete,
}: {
  skill: SkillInfo;
  tools: ToolInfo[];
  /** Move the skill on disk. */
  onRename: (to: SkillIdentity) => Promise<void>;
  /** Show the skill at its new home, once it has moved. */
  onMoved: (to: SkillIdentity) => Promise<void>;
  onDone: () => Promise<void>;
  onDelete: () => Promise<void>;
}) {
  const toolNames = tools.map((t) => t.name);
  const project = useActiveProject();
  const [description, setDescription] = useState(skill.description);
  const [edited, setEdited] = useState(false);
  const [instructions, setInstructions] = useState(skill.instructions);
  const [previewing, setPreviewing] = useState(false);
  const [renaming, setRenaming] = useState<SkillIdentity | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  // Where saves go. Updated the moment a rename lands — inside the save
  // queue, before any later save runs — so no save can recreate the old folder.
  const target = useRef<SkillIdentity>({ name: skill.name, scope: skill.scope });
  const autosave = useAutosave((d: { description: string; instructions: string }) =>
    saveSkill(target.current.scope, target.current.name, d.description, d.instructions),
  );

  function edit(next: { description: string; instructions: string }) {
    setEdited(true);
    setDescription(next.description);
    setInstructions(next.instructions);
    autosave.schedule(next);
  }

  async function act(job: () => Promise<void>) {
    setError(null);
    setBusy(true);
    try {
      await job();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  // Leaving and renaming first write pending typing. A failed save (already
  // shown in the status line) keeps the user here rather than leaving with
  // work unsaved, so it isn't reported twice.
  const flushThen = (job: () => Promise<void>) =>
    act(async () => {
      try {
        await autosave.flush();
      } catch {
        return;
      }
      await job();
    });

  const done = () => flushThen(onDone);

  const applyRename = (to: SkillIdentity) =>
    flushThen(async () => {
      const next = { ...to, name: to.name.trim() };
      // Point saves at the new home the moment the move lands, inside the
      // queue, so even if the reload below fails no save recreates the old one.
      await autosave.run(async () => {
        await onRename(next);
        target.current = next;
      });
      setRenaming(null);
      await onMoved(next);
    });

  // Deleting doesn't need the typing saved (and must work when saves fail).
  // Queued behind any save in flight, then nothing more is written.
  const reallyDelete = () =>
    act(() =>
      autosave.run(async () => {
        await onDelete();
        autosave.discard();
      }),
    );

  const scopeLabel =
    skill.scope === "project" && project ? `project · ${project.name}` : skill.scope;
  // A quiet note beside the name, like a document's "Saved". It stays empty
  // until the first edit; a failure also gets the full message below.
  const status = {
    saved: edited ? "Saved" : "",
    pending: "Saving…",
    saving: "Saving…",
    error: "Not saved",
  }[autosave.status];

  return (
    <div className="settings-page skill-detail">
      <Button size="sm" variant="ghost" className="skill-back" onClick={done} disabled={busy}>
        <ArrowLeft size={15} />
        {skill.name}
      </Button>

      <header className="skill-detail-head">
        <GraduationCap size={20} className="skill-detail-icon" />
        <div className="skill-detail-titles">
          <h3 className="skill-detail-name">
            {skill.name}
            <span className={`skill-scope ${skill.scope}`} title={skill.dir}>
              {scopeLabel}
            </span>
            <span
              className={`skill-save-status ${autosave.status}`}
              role="status"
              aria-live="polite"
              title={autosave.error ?? undefined}
            >
              {autosave.status === "saved" && edited && <Check size={13} />}
              {status}
            </span>
          </h3>
          {skill.draft && !(description.trim() && instructions.trim()) && <DraftNotice />}
        </div>
        <div className="skill-detail-actions">
          {!renaming && (
            <Button
              variant="outline"
              size="sm"
              onClick={() => setRenaming({ name: skill.name, scope: skill.scope })}
            >
              <Pencil size={15} /> Rename or move
            </Button>
          )}
        </div>
      </header>

      <div className="tool-editor">
        {renaming && (
          <form
            className="skill-rename"
            aria-label="Rename or move skill"
            onSubmit={(e) => {
              e.preventDefault();
              void applyRename(renaming);
            }}
          >
            <IdentityFields value={renaming} onChange={setRenaming} />
            <div className="tool-row-actions">
              <Button
                type="submit"
                variant="primary"
                size="sm"
                disabled={!renaming.name.trim() || busy}
              >
                Apply
              </Button>
              <Button type="button" variant="ghost" size="sm" onClick={() => setRenaming(null)}>
                Cancel
              </Button>
            </div>
          </form>
        )}

        {/* Focus leaving either field saves right away (focusout bubbles). */}
        <div className="skill-fields" onBlur={() => void autosave.flush().catch(() => {})}>
          <label className="tool-field">
            <span className="field-name">When should the model use it?</span>
            <input
              className="tool-input"
              value={description}
              onChange={(e) => edit({ description: e.target.value, instructions })}
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
                  <SkillMarkdown text={instructions} toolNames={toolNames} />
                ) : (
                  <p className="muted">Nothing to preview yet.</p>
                )}
              </div>
            ) : (
              <InstructionsEditor
                value={instructions}
                onChange={(next) => edit({ description, instructions: next })}
                tools={tools}
                placeholder={
                  "Markdown the agent follows once the skill loads, e.g.\n\n" +
                  "1. Run `git log --oneline` since the last tag.\n" +
                  "2. Group changes into Added / Fixed / Changed.\n" +
                  "3. Write one crisp line per change — no commit hashes."
                }
              />
            )}
            <span className="tool-field-hint">
              Point the agent at specific tools by naming them in backticks — type{" "}
              <code>`</code> to autocomplete. Known tools highlight in the preview.
            </span>
          </div>
        </div>

        <span className="tool-field-hint">
          Stored at <code>{skill.dir}/SKILL.md</code> — supporting files placed beside it are
          available to the agent.
        </span>

        {error && <span className="save-status err">{error}</span>}
        {autosave.status === "error" && (
          <span className="save-status err">Couldn't save: {autosave.error}</span>
        )}

        <div className="tool-row-actions tool-editor-actions">
          <Button variant="primary" size="sm" onClick={done} disabled={busy}>
            Done
          </Button>
          {confirmDelete ? (
            <Button variant="danger" size="sm" onClick={reallyDelete} disabled={busy}>
              Really delete?
            </Button>
          ) : (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setConfirmDelete(true)}
              title="Remove this skill and its files"
            >
              <Trash2 size={15} /> Delete skill
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}
