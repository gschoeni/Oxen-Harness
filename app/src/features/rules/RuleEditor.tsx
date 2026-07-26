// Writing a rule, and seeing what it would do before it does it.
//
// The tester is the point of the page: a rule is invisible until it fires, so
// the editor shows the match in place and the exact reminder the model would
// receive. Patterns are checked by the agent's own regex engine through
// `check_rule_pattern` — never the browser's, which accepts lookahead and
// backreferences the agent rejects, so a browser-side preview would bless
// patterns that then silently never fire.

import { useEffect, useRef, useState } from "react";
import { Sparkles, Trash2 } from "lucide-react";
import { Button, Spinner } from "../../components/ui";
import { checkRulePattern, draftRule } from "../../lib/ipc";
import type { PatternCheck, RuleSpec } from "../../lib/types";

/** The editor, and the tester that makes an unfired rule legible. */
export function RuleEditor({
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
    // Drop the previous verdict the moment the pattern changes: it described
    // a different pattern, and leaving it up would let a save inside the
    // debounce window store something the engine rejects — the exact failure
    // this page exists to prevent.
    setCheck(null);
    if (!draft.when) return;
    const seq = ++latest.current;
    const timer = setTimeout(() => {
      checkRulePattern(draft.when, sample)
        .then((result) => {
          if (seq === latest.current) setCheck(result);
        })
        .catch((e) => {
          // A failed check is not a passing check.
          if (seq === latest.current) setCheck({ error: String(e), matches: [] });
        });
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
        : check === null
          ? "Checking the pattern…"
          : check.error
            ? `That pattern doesn't compile: ${check.error}`
            : draft.scope.length === 0
              ? "Choose where it watches: tool calls, prose, or both."
              : !draft.message.trim()
                ? "Say what the model should do instead."
                : null;

  const set = (patch: Partial<RuleSpec>) => setDraft({ ...draft, ...patch });

  return (
    <div className={`rule-row editing ${draft.interrupt ? "interrupts" : "reminds"}`}>
      <Describe
        onDrafted={({ sample: drafted, ...fields }) => {
          setDraft({ ...draft, ...fields });
          // The model's example becomes the tester's sample, so the new rule
          // shows itself catching something the moment it lands.
          if (drafted) setSample(drafted);
        }}
      />
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
              title={
                draft.scope.includes(scope) && draft.scope.length === 1
                  ? "A rule has to watch something — turn the other one on first"
                  : undefined
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
  const mirror = useRef<HTMLPreElement>(null);
  return (
    <div className="rule-tester">
      <div className="rule-tester-head">
        <span>Try it</span>
        {/* An empty pattern hasn't been evaluated — saying "no match" there
            reads as a broken tester rather than an unfinished rule. */}
        {!draft.when ? (
          <span className="rule-verdict">add a pattern above to see what it catches</span>
        ) : hits.length > 0 ? (
          <span className="rule-verdict hit">
            {hits.length === 1 ? "1 match" : `${hits.length} matches`} — this rule would fire
          </span>
        ) : (
          <span className="rule-verdict">no match — the model would carry on</span>
        )}
      </div>
      <div className="rule-sample-wrap">
        <pre className="rule-sample-mirror" aria-hidden="true" ref={mirror}>
          {highlight(sample, hits)}
        </pre>
        <textarea
          className="rule-sample"
          value={sample}
          onChange={(e) => setSample(e.target.value)}
          // The mirror is a separate layer, so it has to follow the textarea's
          // scroll or the highlight drifts onto unrelated lines.
          onScroll={(e) => {
            if (mirror.current) mirror.current.scrollTop = e.currentTarget.scrollTop;
          }}
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

/** Say what you want in plain language and let the model write the rule.
 *
 *  Regexes are the part of this feature people bounce off, and the model is
 *  good at them — but only if the result is checked. The draft that arrives
 *  here has already compiled, caught its own example, and left its
 *  counter-example alone; anything less comes back as an error, so this can
 *  fill the form without asking the user to audit it first.
 */
function Describe({
  onDrafted,
}: {
  onDrafted: (fields: Partial<RuleSpec> & { sample?: string }) => void;
}) {
  const [want, setWant] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function write() {
    if (!want.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      const drafted = await draftRule(want.trim());
      onDrafted({
        name: drafted.name,
        when: drafted.pattern,
        scope: drafted.scopes,
        message: drafted.message,
        interrupt: drafted.interrupt,
        // The model's own example becomes the tester's sample, so the rule
        // arrives with its proof visible rather than asserted.
        sample: drafted.example_match,
      });
      setWant("");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="rule-describe">
      <label className="rule-field">
        <span>Describe it, and I'll write it</span>
        <div className="rule-describe-row">
          <input
            value={want}
            onChange={(e) => setWant(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && write()}
            placeholder="don't let it delete database migrations"
            disabled={busy}
          />
          <Button onClick={write} disabled={!want.trim() || busy}>
            {busy ? <Spinner /> : <Sparkles size={14} />} Write it
          </Button>
        </div>
      </label>
      {error && <span className="rule-describe-error">{error}</span>}
    </div>
  );
}
