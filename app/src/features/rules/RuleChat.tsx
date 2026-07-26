// Writing a rule by talking about it.
//
// The first version of this was a text box and a spinner: you typed what you
// wanted, watched a circle turn, and either the form filled in or it didn't.
// You couldn't tell whether the model was thinking, stuck, or about to hand
// back something wrong — and if it wasn't quite right, your only move was to
// start over.
//
// This is a conversation instead. The model's sentence streams in as it
// writes, the rule it produces appears as a card in the thread (and fills the
// form beside it), the check against its own examples is shown rather than
// claimed, and the next message revises what's on the table. The form and the
// tester stay visible throughout, so the conversation is *about* something you
// can see.

import { useEffect, useRef, useState } from "react";
import { Check, CornerDownLeft, Sparkles, X } from "lucide-react";
import { draftRule, onRuleDraft } from "../../lib/ipc";
import type { DraftTurn, RuleSpec } from "../../lib/types";

/** What the model produced, shown in the thread and applied to the form. */
interface Proposal {
  name: string;
  pattern: string;
  interrupt: boolean;
  scopes: string[];
  catches: string;
  ignores: string;
  /** True when this is the rule already on disk rather than a fresh proposal. */
  saved?: boolean;
}

/** Rebuild the conversation for a rule being reopened.
 *
 *  Two things come back: the ask that produced it, shown in the thread, and the
 *  rule itself as conversation history — so the first follow-up revises what's
 *  saved instead of writing something unrelated. A rule written by hand has no
 *  ask, but still gets the history, which is what makes "make it stricter" work
 *  on a rule the model never saw. */
function resume(rule?: RuleSpec): { messages: Message[]; history: DraftTurn[] } {
  if (!rule?.when) return { messages: [], history: [] };
  const proposal: Proposal = {
    name: rule.name,
    pattern: rule.when,
    interrupt: rule.interrupt,
    scopes: rule.scope,
    catches: rule.sample ?? "",
    ignores: "",
    saved: true,
  };
  const asked = rule.prompt?.trim();
  return {
    messages: asked ? [{ kind: "earlier", text: asked, proposal }] : [],
    history: [
      {
        asked: asked || "Write this rule.",
        said: "",
        rule: JSON.stringify({
          name: rule.name,
          pattern: rule.when,
          scopes: rule.scope,
          message: rule.message,
          interrupt: rule.interrupt,
          example_match: rule.sample ?? "",
          example_miss: "",
        }),
      },
    ],
  };
}

type Message =
  | { kind: "you"; text: string }
  | { kind: "model"; text: string; proposal?: Proposal; retry?: string }
  | { kind: "failed"; text: string }
  // A conversation restored from a saved rule: the ask is the user's own, the
  // card is what's on disk. Neither is put in the model's mouth — its note
  // isn't saved, so it isn't reconstructed.
  | { kind: "earlier"; text: string; proposal: Proposal };

/** Follow-ups worth one click, offered once there's a rule to revise. */
const NUDGES = [
  "make it stricter",
  "make it less likely to false-positive",
  "watch prose too",
  "just remind, don't interrupt",
];

export function RuleChat({
  existing,
  onProposal,
}: {
  /** The rule being edited, so reopening one resumes its conversation rather
   *  than starting a stranger's. */
  existing?: RuleSpec;
  /** Applies the model's proposal to the fields below. */
  onProposal: (fields: Partial<RuleSpec> & { sample?: string }) => void;
}) {
  // Seeded once per mount: the editor is keyed by rule, so opening a different
  // one brings its own conversation.
  const [resumed] = useState(() => resume(existing));
  const [messages, setMessages] = useState<Message[]>(resumed.messages);
  const [want, setWant] = useState("");
  const [streaming, setStreaming] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const history = useRef<DraftTurn[]>(resumed.history);
  const thread = useRef<HTMLDivElement>(null);

  // Tokens arrive on a channel while the command is in flight; the partial
  // sentence is the point — it's how you know something is happening.
  useEffect(() => {
    const stop = onRuleDraft((e) => {
      if (e.retry) {
        setMessages((m) => [...m, { kind: "model", text: e.retry!, retry: e.retry }]);
        setStreaming("");
        return;
      }
      // Everything up to the JSON is the model talking; stop showing it once
      // the object starts.
      setStreaming((s) => {
        const next = (s ?? "") + (e.delta ?? "");
        const brace = next.indexOf("{");
        return brace >= 0 ? next.slice(0, brace) : next;
      });
    });
    return () => void stop.then((f) => f());
  }, []);

  useEffect(() => {
    // Plain assignment rather than scrollTo: jsdom has no scrollTo, and the
    // behaviour we want (pin to the newest line) doesn't need smoothing.
    if (thread.current) thread.current.scrollTop = thread.current.scrollHeight;
  }, [messages, streaming]);

  async function send(text: string) {
    const asked = text.trim();
    if (!asked || busy) return;
    setWant("");
    setBusy(true);
    setStreaming("");
    setMessages((m) => [...m, { kind: "you", text: asked }]);
    try {
      const out = await draftRule(asked, history.current);
      const proposal: Proposal = {
        name: out.rule.name,
        pattern: out.rule.pattern,
        interrupt: out.rule.interrupt,
        scopes: out.rule.scopes,
        catches: out.rule.example_match,
        ignores: out.rule.example_miss,
      };
      history.current = [
        ...history.current,
        { asked, said: out.note, rule: JSON.stringify(out.rule) },
      ];
      setMessages((m) => [...m, { kind: "model", text: out.note, proposal }]);
      onProposal({
        name: out.rule.name,
        when: out.rule.pattern,
        scope: out.rule.scopes,
        message: out.rule.message,
        interrupt: out.rule.interrupt,
        sample: out.rule.example_match,
        // Saved with the rule, so this conversation is here again next time.
        prompt: out.prompt,
      });
    } catch (e) {
      setMessages((m) => [...m, { kind: "failed", text: String(e) }]);
    } finally {
      setStreaming(null);
      setBusy(false);
    }
  }

  const started = messages.length > 0;

  return (
    <div className="rule-chat">
      <div className="rule-chat-head">
        <Sparkles size={13} />
        <span>Write it with the model</span>
        <span className="rule-chat-sub">
          {started ? "keep going — each message revises the rule" : "say what you want to prevent"}
        </span>
      </div>

      {started && (
        <div className="rule-chat-thread" ref={thread}>
          {messages.map((m, i) => (
            <Bubble key={i} message={m} />
          ))}
          {streaming !== null && (
            <div className="rule-msg model">
              <span className="rule-msg-who">model</span>
              <p className="rule-msg-text">
                {streaming || "…"}
                <span className="rule-caret" />
              </p>
            </div>
          )}
        </div>
      )}

      <div className="rule-chat-composer">
        <input
          value={want}
          onChange={(e) => setWant(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && send(want)}
          placeholder={
            started
              ? "also catch mv, and don't fire on paths inside the repo"
              : "don't remove files from outside the project without asking"
          }
          disabled={busy}
        />
        <button
          type="button"
          className="rule-chat-send"
          onClick={() => send(want)}
          disabled={!want.trim() || busy}
          aria-label="Send"
        >
          <CornerDownLeft size={14} />
        </button>
      </div>

      {started && !busy && (
        <div className="rule-chat-nudges">
          {NUDGES.map((n) => (
            <button key={n} type="button" className="rule-chip" onClick={() => send(n)}>
              {n}
            </button>
          ))}
        </div>
      )}
      {!started && (
        <p className="rule-chat-note">
          It writes the pattern, checks it against its own example, and fills in the fields
          below — which stay yours to edit.
        </p>
      )}
    </div>
  );
}

function Bubble({ message }: { message: Message }) {
  if (message.kind === "earlier") {
    return (
      <>
        <div className="rule-msg you earlier">
          <span className="rule-msg-who">you · earlier</span>
          <p className="rule-msg-text">{message.text}</p>
        </div>
        <div className="rule-msg saved">
          <span className="rule-msg-who">saved</span>
          <ProposalCard proposal={message.proposal} />
        </div>
      </>
    );
  }
  if (message.kind === "you") {
    return (
      <div className="rule-msg you">
        <span className="rule-msg-who">you</span>
        <p className="rule-msg-text">{message.text}</p>
      </div>
    );
  }
  if (message.kind === "failed") {
    return (
      <div className="rule-msg failed">
        <span className="rule-msg-who">model</span>
        <p className="rule-msg-text">Couldn't write that one — {message.text}</p>
      </div>
    );
  }
  return (
    <div className={`rule-msg model ${message.retry ? "retrying" : ""}`}>
      <span className="rule-msg-who">model</span>
      <p className="rule-msg-text">{message.text}</p>
      {message.proposal && <ProposalCard proposal={message.proposal} />}
    </div>
  );
}

/** The rule the model wrote, with the check that ran on it. */
function ProposalCard({ proposal }: { proposal: Proposal }) {
  return (
    <div className={`rule-proposal ${proposal.interrupt ? "interrupts" : "reminds"}`}>
      <div className="rule-proposal-head">
        <code>{proposal.pattern}</code>
        <span className="rule-effect">{proposal.interrupt ? "interrupts" : "reminds"}</span>
      </div>
      {/* Shown, not claimed: these two ran through the same engine the rule
          will run on. */}
      <div className="rule-proposal-checks">
        {/* A rule written by hand has no example, so there is nothing to show
            catching — an empty `catches` line would read as a failed check. */}
        {proposal.catches &&
          // A fresh proposal was checked before it was offered, so it can claim
          // to catch its example. A saved rule's example is only what was kept
          // with it — the tester below is what actually runs it, so this says
          // no more than it knows.
          (proposal.saved ? (
            <span className="sample">
              tested against <code>{proposal.catches}</code>
            </span>
          ) : (
            <span className="hit">
              <Check size={11} /> catches <code>{proposal.catches}</code>
            </span>
          ))}
        {proposal.ignores && (
          <span className="miss">
            <X size={11} /> ignores <code>{proposal.ignores}</code>
          </span>
        )}
      </div>
      <span className="rule-proposal-applied">
        {proposal.saved ? "this is the rule below" : "applied to the fields below"}
      </span>
    </div>
  );
}
