// Writing a rule, and seeing what it would do before it does it.
//
// The tester is the point of the page: a rule is invisible until it fires, so
// the editor shows the match in place and the exact reminder the model would
// receive. Patterns are checked by the agent's own regex engine through
// `check_rule_pattern` — never the browser's, which accepts lookahead and
// backreferences the agent rejects, so a browser-side preview would bless
// patterns that then silently never fire.

import { useEffect, useRef, useState } from "react";
import { Trash2 } from "lucide-react";
import { Button } from "../../components/ui";
import { checkRulePattern } from "../../lib/ipc";
import type { PatternCheck, RuleSpec } from "../../lib/types";
import { RuleChat } from "./RuleChat";

/** What the tester holds before anyone has chosen a sample. Recognisably an
 *  example rather than your code, so an unrelated "no match" doesn't read as a
 *  broken rule. */
const PLACEHOLDER_SAMPLE = 'let value = config.get("port").unwrap();';

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
  // The rule's own sample when it has one — a rule about `kill` should not
  // open with a sample about `.unwrap()` and report "no match", which reads as
  // a broken rule rather than an irrelevant example.
  const [sample, setSample] = useState(draft.sample || PLACEHOLDER_SAMPLE);
  // Only a sample someone chose is worth saving; the placeholder isn't, or
  // every hand-written rule would come back paired with an unrelated line.
  const [chosen, setChosen] = useState(Boolean(draft.sample));
  const [check, setCheck] = useState<PatternCheck | null>(null);
  // The rule as it was when the editor opened, which is what the conversation
  // resumes from — the live draft changes under it as the model works.
  const original = useRef(draft);
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
      <RuleChat
        existing={original.current}
        onProposal={({ sample: proposed, ...fields }) => {
          setDraft({ ...draft, ...fields, sample: proposed ?? draft.sample });
          // The model's example becomes the tester's sample, so a proposed
          // rule shows itself catching something the moment it lands.
          if (proposed) {
            setSample(proposed);
            setChosen(true);
          }
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

      <Tester
        sample={sample}
        setSample={(s) => {
          setSample(s);
          setChosen(true);
        }}
        check={check}
        draft={draft}
      />

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
            onClick={() =>
              onSave({
                ...draft,
                name: draft.name.trim(),
                sample: chosen ? sample : draft.sample,
              })
            }
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
        ) : check?.error ? (
          // Nothing was evaluated, so "no match" would be a verdict on a run
          // that never happened — the same lie as reporting it on an empty
          // pattern.
          <span className="rule-verdict invalid">that pattern doesn't compile — nothing runs</span>
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
