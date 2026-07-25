// One rule at rest: what it watches, where, and what it does about it.
//
// Whether a rule *interrupts* is the only choice here that throws work away,
// so it's carried by the row's left edge as well as a label — which is what
// makes the list readable straight down.

import { FolderGit2, Zap } from "lucide-react";
import type { RuleSpec } from "../../lib/types";
import { ToolSwitch } from "../tools/ToolSwitch";

/** One rule at rest: what it watches, where, and what it does about it. */
export function RuleRow({
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
