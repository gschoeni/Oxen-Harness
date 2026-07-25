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

import { useCallback, useEffect, useRef, useState } from "react";
import { FolderGit2, Plus, Trash2, Zap } from "lucide-react";
import { Button } from "../../components/ui";
import { checkRulePattern, listRules, saveRules } from "../../lib/ipc";
import type { PatternCheck, RuleSpec } from "../../lib/types";
import { ToolSwitch } from "../tools/ToolSwitch";
import "../tools/tools.css";
import "./rules.css";

/** Rules worth having on day one, offered when there are none. */
const STARTERS: RuleSpec[] = [
  {
    name: "no-unwrap",
    when: "\\.unwrap\\(\\)",
    scope: ["tool"],
    message:
      "This project doesn't use `.unwrap()` outside tests — return a Result, or use `expect` with a reason that says what invariant is being relied on.",
    interrupt: true,
    repeat: "once",
    enabled: true,
  },
  {
    name: "leave-generated-alone",
    when: "generated/",
    scope: ["tool"],
    message:
      "Files under generated/ are produced by the build. Change the generator or its input instead, then re-run it.",
    interrupt: true,
    repeat: "once",
    enabled: true,
  },
  {
    name: "no-force-push",
    when: "push\\s+--force|push\\s+-f\\b",
    scope: ["tool"],
    message: "Don't force-push. If history needs fixing, say what you'd do and let me decide.",
    interrupt: true,
    repeat: "once",
    enabled: true,
  },
];

const blank = (): RuleSpec => ({
  name: "",
  when: "",
  scope: ["tool"],
  message: "",
  interrupt: true,
  repeat: "once",
  enabled: true,
});

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

  // Every change writes the whole set, so edit, delete, and toggle are one path.
  async function persist(next: RuleSpec[]) {
    setUser(next);
    try {
      await saveRules(next);
      setError(null);
    } catch (e) {
      setError(String(e));
      load();
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
    setEditing(null);
    setDraft(null);
    await persist(next);
  }

  const rules = user ?? [];
  const firing = rules.filter((r) => r.enabled).length;

  return (
    <div className="settings-page">
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

/** One rule at rest: what it watches, where, and what it does about it. */
function RuleRow({
  rule,
  readOnly,
  onEdit,
  onToggle,
}: {
  rule: RuleSpec;
  readOnly?: boolean;
  onEdit?: () => void;
  onToggle?: (enabled: boolean) => void;
}) {
  return (
    <div
      className={`rule-row ${rule.interrupt ? "interrupts" : "reminds"} ${
        rule.enabled ? "" : "off"
      }`}
    >
      <button
        type="button"
        className="rule-row-main"
        onClick={onEdit}
        disabled={readOnly}
        aria-label={readOnly ? undefined : `Edit ${rule.name}`}
      >
        <div className="rule-row-head">
          <span className="rule-name">{rule.name}</span>
          {rule.interrupt ? (
            <span className="rule-effect interrupts">
              <Zap size={11} /> interrupts
            </span>
          ) : (
            <span className="rule-effect">reminds</span>
          )}
          {readOnly && (
            <span className="rule-effect">
              <FolderGit2 size={11} /> repo
            </span>
          )}
        </div>
        <div className="rule-watch">
          <code className="rule-pattern">{rule.when}</code>
          <span className="rule-scopes">in {scopeLabel(rule.scope)}</span>
        </div>
        <p className="rule-message">{rule.message}</p>
      </button>
      {!readOnly && onToggle && (
        <ToolSwitch name={rule.name} enabled={rule.enabled} onToggle={(_, on) => onToggle(on)} />
      )}
    </div>
  );
}

function scopeLabel(scope: string[]): string {
  if (scope.length === 0) return "prose + tool calls";
  return scope
    .map((s) => (s === "tool" ? "tool calls" : s === "text" ? "prose" : s))
    .join(" + ");
}

/** The editor, and the tester that makes an unfired rule legible. */
function RuleEditor({
  draft,
  setDraft,
  taken,
  onSave,
  onCancel,
  onDelete,
}: {
  draft: RuleSpec;
  setDraft: (r: RuleSpec) => void;
  taken: string[];
  onSave: (r: RuleSpec) => void;
  onCancel: () => void;
  onDelete?: () => void;
}) {
  const [sample, setSample] = useState(
    "let value = config.get(\"port\").unwrap();",
  );
  const [check, setCheck] = useState<PatternCheck | null>(null);
  // Guards against an out-of-order response overwriting a newer check.
  const latest = useRef(0);

  useEffect(() => {
    if (!draft.when) {
      setCheck(null);
      return;
    }
    const seq = ++latest.current;
    const timer = setTimeout(() => {
      checkRulePattern(draft.when, sample)
        .then((result) => {
          if (seq === latest.current) setCheck(result);
        })
        .catch(() => {});
    }, 120);
    return () => clearTimeout(timer);
  }, [draft.when, sample]);

  const duplicate = taken.includes(draft.name.trim());
  const problem = !draft.name.trim()
    ? "Give the rule a name."
    : duplicate
      ? "You already have a rule with that name."
      : !draft.when
        ? "Add a pattern to watch for."
        : check?.error
          ? `That pattern doesn't compile: ${check.error}`
          : !draft.message.trim()
            ? "Say what the model should do instead."
            : null;

  const set = (patch: Partial<RuleSpec>) => setDraft({ ...draft, ...patch });

  return (
    <div className={`rule-row editing ${draft.interrupt ? "interrupts" : "reminds"}`}>
      <div className="rule-fields">
        <label className="rule-field">
          <span>Name</span>
          <input
            value={draft.name}
            onChange={(e) => set({ name: e.target.value })}
            placeholder="no-unwrap"
            spellCheck={false}
            autoFocus
          />
        </label>
        <label className="rule-field">
          <span>Watch for</span>
          <input
            className={`rule-pattern-input ${check?.error ? "invalid" : ""}`}
            value={draft.when}
            onChange={(e) => set({ when: e.target.value })}
            placeholder="\.unwrap\(\)"
            spellCheck={false}
          />
        </label>
      </div>

      <label className="rule-field">
        <span>Tell the model</span>
        <textarea
          value={draft.message}
          onChange={(e) => set({ message: e.target.value })}
          placeholder="Return a Result instead — this project doesn't unwrap outside tests."
          rows={2}
        />
      </label>

      <div className="rule-options">
        <div className="rule-option-group" role="group" aria-label="Where it watches">
          <span className="rule-option-label">Watches</span>
          {(["tool", "text"] as const).map((scope) => (
            <button
              key={scope}
              type="button"
              className={`rule-chip ${draft.scope.includes(scope) ? "on" : ""}`}
              aria-pressed={draft.scope.includes(scope)}
              onClick={() =>
                set({
                  scope: draft.scope.includes(scope)
                    ? draft.scope.filter((s) => s !== scope)
                    : [...draft.scope, scope],
                })
              }
            >
              {scope === "tool" ? "tool calls" : "prose"}
            </button>
          ))}
        </div>
        <div className="rule-option-group" role="group" aria-label="What it does on a match">
          <span className="rule-option-label">On a match</span>
          <button
            type="button"
            className={`rule-chip ${draft.interrupt ? "on" : ""}`}
            aria-pressed={draft.interrupt}
            onClick={() => set({ interrupt: true })}
          >
            interrupt
          </button>
          <button
            type="button"
            className={`rule-chip ${draft.interrupt ? "" : "on"}`}
            aria-pressed={!draft.interrupt}
            onClick={() => set({ interrupt: false })}
          >
            remind
          </button>
        </div>
      </div>

      <Tester sample={sample} setSample={setSample} check={check} draft={draft} />

      <div className="rule-actions">
        {onDelete && (
          <button type="button" className="rule-delete" onClick={onDelete}>
            <Trash2 size={14} /> Delete
          </button>
        )}
        {problem && <span className="rule-problem">{problem}</span>}
        <div className="rule-actions-buttons">
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button
            onClick={() => onSave({ ...draft, name: draft.name.trim() })}
            disabled={problem !== null}
          >
            Save rule
          </Button>
        </div>
      </div>
    </div>
  );
}

/** Paste what the model might write; see what the rule does about it. */
function Tester({
  sample,
  setSample,
  check,
  draft,
}: {
  sample: string;
  setSample: (s: string) => void;
  check: PatternCheck | null;
  draft: RuleSpec;
}) {
  const hits = check?.error ? [] : (check?.matches ?? []);
  return (
    <div className="rule-tester">
      <div className="rule-tester-head">
        <span>Try it</span>
        {hits.length > 0 ? (
          <span className="rule-verdict hit">
            {hits.length === 1 ? "1 match" : `${hits.length} matches`} — this rule would fire
          </span>
        ) : (
          <span className="rule-verdict">no match — the model would carry on</span>
        )}
      </div>
      <div className="rule-sample-wrap">
        <pre className="rule-sample-mirror" aria-hidden="true">
          {highlight(sample, hits)}
        </pre>
        <textarea
          className="rule-sample"
          value={sample}
          onChange={(e) => setSample(e.target.value)}
          spellCheck={false}
          rows={2}
          aria-label="Sample model output"
        />
      </div>
      {hits.length > 0 && (
        <div className="rule-consequence">
          <div className="rule-reminder">
            {`<system-reminder rule="${draft.name || "…"}">`}
            <br />
            {draft.message || "…"}
            <br />
            {"</system-reminder>"}
          </div>
          <p className="rule-consequence-note">
            {draft.interrupt
              ? "The reply in flight is thrown away and the model tries again with this."
              : "The model finishes, then gets this before its next step."}
          </p>
        </div>
      )}
    </div>
  );
}

/** Split `text` on the matched byte ranges so matches can be marked. Ranges
 *  come from the Rust engine as byte offsets; the encoder maps them back to
 *  the string the textarea holds. */
function highlight(text: string, matches: [number, number][]) {
  if (matches.length === 0) return text;
  const bytes = new TextEncoder().encode(text);
  const decoder = new TextDecoder();
  const parts: React.ReactNode[] = [];
  let at = 0;
  matches.forEach(([start, end], i) => {
    if (start > at) parts.push(decoder.decode(bytes.slice(at, start)));
    parts.push(
      <mark key={i} className="rule-hit">
        {decoder.decode(bytes.slice(start, end))}
      </mark>,
    );
    at = end;
  });
  if (at < bytes.length) parts.push(decoder.decode(bytes.slice(at)));
  return parts;
}
