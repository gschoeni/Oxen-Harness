// The Rules settings subpage: stream rules that watch the model's output and
// correct it when a pattern matches.
//
// A rule is invisible until it fires, which makes it the hardest kind of
// setting to trust. So the editor is built around a tester: paste something the
// model might write, watch the pattern catch it, and read the exact reminder
// the model would get. The matching runs through the agent's own regex engine
// (`check_rule_pattern`), never the browser's — JavaScript accepts lookahead
// and backreferences that engine doesn't, so a browser-side preview would bless
// patterns that then silently never fire.

import { useCallback, useEffect, useState } from "react";
import { Plus } from "lucide-react";
import { listRules, saveRules } from "../../lib/ipc";
import { TeachingNav } from "../settings/TeachingNav";
import type { RuleSpec } from "../../lib/types";
import { RuleEditor } from "./RuleEditor";
import { RuleRow } from "./RuleRow";
import { blank, STARTERS } from "./starters";
import "../tools/tools.css";
import "./rules.css";

export function RulesPage() {
  const [user, setUser] = useState<RuleSpec[] | null>(null);
  const [project, setProject] = useState<RuleSpec[]>([]);
  const [projectPath, setProjectPath] = useState(".oxen-harness/rules.json");
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState<RuleSpec | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(
    () =>
      listRules()
        .then((sets) => {
          setUser(sets.user);
          setProject(sets.project);
          setProjectPath(sets.project_path);
        })
        .catch((e) => setError(String(e))),
    [],
  );

  useEffect(() => {
    load();
  }, [load]);

  // Every change writes the whole set, so edit, delete, and toggle are one
  // path. Returns whether it stuck, so callers holding unsaved input can keep
  // holding it.
  async function persist(next: RuleSpec[]): Promise<boolean> {
    setUser(next);
    try {
      await saveRules(next);
      setError(null);
      return true;
    } catch (e) {
      setError(String(e));
      load();
      return false;
    }
  }

  function startEdit(rule: RuleSpec) {
    setEditing(rule.name);
    setDraft({ ...rule });
  }

  function startNew() {
    setEditing("");
    setDraft(blank());
  }

  async function commit(rule: RuleSpec) {
    const rules = user ?? [];
    const at = rules.findIndex((r) => r.name === editing);
    const next = at >= 0 ? rules.map((r, i) => (i === at ? rule : r)) : [...rules, rule];
    // The editor stays open until the write lands: a failed save used to
    // reload from disk over the optimistic entry, losing what was just typed.
    if (await persist(next)) {
      setEditing(null);
      setDraft(null);
    }
  }

  const rules = user ?? [];
  const firing = rules.filter((r) => r.enabled).length;

  return (
    <div className="settings-page">
      <TeachingNav current="rules" />
      <section className="settings-section">
        <div className="settings-label">
          Your rules{user && rules.length > 0 && ` · ${firing} of ${rules.length} on`}
        </div>
        <p className="hint">
          A rule watches what the model writes and corrects it the moment a pattern matches — an
          interrupting rule throws the reply away and asks again, so the correction lands before
          the work does. Unlike an instruction in the system prompt, a rule costs nothing until
          it matches. Changes apply to <strong>new and resumed chats</strong>.
        </p>
        {error && <span className="save-status err">{error}</span>}

        {user && rules.length === 0 && editing === null && (
          <div className="rule-empty">
            <p>No rules yet. Start with one of these, or write your own.</p>
            <div className="rule-starters">
              {STARTERS.map((starter) => (
                <button
                  key={starter.name}
                  type="button"
                  className="rule-starter"
                  onClick={() => persist([...(user ?? []), starter])}
                >
                  <code>{starter.when}</code>
                  <span>{starter.name}</span>
                </button>
              ))}
            </div>
          </div>
        )}

        <div className="rule-list">
          {rules.map((rule) =>
            editing === rule.name && draft ? (
              <RuleEditor
                key={rule.name}
                draft={draft}
                setDraft={setDraft}
                taken={rules.filter((r) => r.name !== rule.name).map((r) => r.name)}
                onSave={commit}
                onCancel={() => {
                  setEditing(null);
                  setDraft(null);
                }}
                onDelete={() => {
                  setEditing(null);
                  setDraft(null);
                  persist(rules.filter((r) => r.name !== rule.name));
                }}
              />
            ) : (
              <RuleRow
                key={rule.name}
                rule={rule}
                onEdit={() => startEdit(rule)}
                onToggle={(enabled) =>
                  persist(rules.map((r) => (r.name === rule.name ? { ...r, enabled } : r)))
                }
              />
            ),
          )}
          {editing === "" && draft && (
            <RuleEditor
              draft={draft}
              setDraft={setDraft}
              taken={rules.map((r) => r.name)}
              onSave={commit}
              onCancel={() => {
                setEditing(null);
                setDraft(null);
              }}
            />
          )}
        </div>

        {editing === null && (
          <button type="button" className="tool-add" onClick={startNew}>
            <Plus size={15} /> New rule
          </button>
        )}
      </section>

      {project.length > 0 && (
        <section className="settings-section">
          <div className="settings-label">This project's rules · {project.length}</div>
          <p className="hint">
            Committed to the repository in <code>{projectPath}</code>, so everyone working here
            gets them. Edit the file to change them; a rule here overrides one of yours with the
            same name.
          </p>
          <div className="rule-list">
            {project.map((rule) => (
              <RuleRow key={rule.name} rule={rule} readOnly />
            ))}
          </div>
        </section>
      )}
    </div>
  );
}
