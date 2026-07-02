// The skill-instructions editor and renderer, both aware of the tool
// vocabulary. Skills direct the agent to tools by naming them in backticks
// (e.g. "run `git` with operation=status") — the model connects the name to
// the registered tool when the skill loads. This module makes that convention
// visible: typing a backtick autocompletes tool names, known references render
// as highlighted chips, and near-miss names (snake_case but unknown) get a
// typo warning instead of failing silently at runtime.

import { useRef, useState, type KeyboardEvent } from "react";
import { AlertTriangle, Wrench } from "lucide-react";
import { Markdown } from "../../components/ui/Markdown";

/** Markdown with tool-aware `code` spans: content exactly matching a known
 *  tool name renders as a highlighted tool reference. */
export function SkillMarkdown({ text, toolNames }: { text: string; toolNames: string[] }) {
  const known = new Set(toolNames);
  return (
    <Markdown
      text={text}
      components={{
        code: ({ children, className, ...props }) => {
          const content = typeof children === "string" ? children : "";
          if (known.has(content)) {
            return (
              <code className="tool-ref" title="A tool the agent can call">
                {content}
              </code>
            );
          }
          return (
            <code className={className} {...props}>
              {children}
            </code>
          );
        },
      }}
    />
  );
}

/** Every backticked token in `text` that reads like a tool name, split into
 *  known tools and probable typos. Only snake_case tokens (containing `_`)
 *  are flagged as unknown — plain code spans like `git log` shouldn't warn. */
export function toolReferences(
  text: string,
  toolNames: string[],
): { known: string[]; unknown: string[] } {
  const knownSet = new Set(toolNames);
  const known = new Set<string>();
  const unknown = new Set<string>();
  for (const [, token] of text.matchAll(/`([a-z][a-z0-9_]*)`/g)) {
    if (knownSet.has(token)) known.add(token);
    else if (token.includes("_")) unknown.add(token);
  }
  return { known: [...known], unknown: [...unknown] };
}

/** The token being typed when the caret sits inside an open backtick, e.g.
 *  `` `rea|`` → { start, query: "rea" }. Null when not in a tool token. */
function openToken(text: string, caret: number): { start: number; query: string } | null {
  const lineStart = text.lastIndexOf("\n", caret - 1) + 1;
  for (let i = caret - 1; i >= lineStart; i--) {
    const c = text[i];
    if (c === "`") {
      // Backtick parity on the line decides whether this tick OPENS a span
      // (complete here) or CLOSES one (`read_file`| — don't reopen).
      const ticksBefore = (text.slice(lineStart, i).match(/`/g) ?? []).length;
      if (ticksBefore % 2 === 1) return null;
      return { start: i + 1, query: text.slice(i + 1, caret) };
    }
    if (!/[a-z0-9_]/.test(c)) return null;
  }
  return null;
}

/** The instructions textarea with tool-name autocomplete: typing a backtick
 *  offers the registered tools; Enter/Tab/click inserts the completed
 *  `` `name` `` reference. */
export function InstructionsEditor({
  value,
  onChange,
  toolNames,
  placeholder,
}: {
  value: string;
  onChange: (value: string) => void;
  toolNames: string[];
  placeholder?: string;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const [token, setToken] = useState<{ start: number; query: string } | null>(null);
  const [active, setActive] = useState(0);

  const matches = token
    ? toolNames.filter((n) => n.startsWith(token.query) && n !== token.query).slice(0, 6)
    : [];
  const open = matches.length > 0;

  /** Re-derive the autocomplete token from the live caret position. */
  function refresh(el: HTMLTextAreaElement) {
    setToken(openToken(el.value, el.selectionStart));
    setActive(0);
  }

  function accept(name: string) {
    const el = ref.current;
    if (!el || !token) return;
    const caret = el.selectionStart;
    const after = value.slice(caret);
    const needsClosing = !after.startsWith("`");
    const next = value.slice(0, token.start) + name + (needsClosing ? "`" : "") + after;
    const caretPos = token.start + name.length + 1; // just past the closing tick
    onChange(next);
    setToken(null);
    requestAnimationFrame(() => {
      el.focus();
      el.setSelectionRange(caretPos, caretPos);
    });
  }

  function onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (!open) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => (a + 1) % matches.length);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => (a - 1 + matches.length) % matches.length);
    } else if (e.key === "Enter" || e.key === "Tab") {
      e.preventDefault();
      accept(matches[active]);
    } else if (e.key === "Escape") {
      setToken(null);
    }
  }

  const refs = toolReferences(value, toolNames);

  return (
    <div className="skill-editor-area">
      <textarea
        ref={ref}
        className="tool-desc-edit skill-instructions"
        rows={16}
        value={value}
        onChange={(e) => {
          onChange(e.target.value);
          refresh(e.target);
        }}
        onKeyDown={onKeyDown}
        onKeyUp={(e) => refresh(e.currentTarget)}
        onClick={(e) => refresh(e.currentTarget)}
        onBlur={() => setToken(null)}
        placeholder={placeholder}
        spellCheck={false}
        aria-label="Instructions markdown"
      />

      {open && (
        <ul className="skill-autocomplete" role="listbox" aria-label="Tool name suggestions">
          {matches.map((name, i) => (
            <li key={name}>
              <button
                type="button"
                role="option"
                aria-selected={i === active}
                className={i === active ? "active" : ""}
                // onMouseDown so the click lands before the textarea's blur.
                onMouseDown={(e) => {
                  e.preventDefault();
                  accept(name);
                }}
              >
                <Wrench size={12} />
                {name}
              </button>
            </li>
          ))}
        </ul>
      )}

      {(refs.known.length > 0 || refs.unknown.length > 0) && (
        <div className="skill-refs" aria-label="Referenced tools">
          <span className="skill-refs-label">References</span>
          {refs.known.map((name) => (
            <span key={name} className="skill-ref-chip">
              <Wrench size={11} />
              {name}
            </span>
          ))}
          {refs.unknown.map((name) => (
            <span
              key={name}
              className="skill-ref-chip unknown"
              title={`\`${name}\` doesn't match any tool — a typo, or a tool that isn't registered.`}
            >
              <AlertTriangle size={11} />
              {name}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
