// The Permissions settings subpage: how eagerly the agent asks before running
// tools, plus the allow/deny command rules. The mode and rules persist to
// permissions.json (global) and the active project's .oxen-harness/
// permissions.json; like tool preferences they apply when an agent is built,
// so changes reach new and resumed chats. Circuit breakers (rm -rf /, .git
// writes, …) are hard limits and are not configurable here or anywhere.

import { useCallback, useEffect, useState } from "react";
import { ShieldAlert, X } from "lucide-react";
import {
  addPermissionRule,
  getPermissions,
  removePermissionRule,
  setPermissionMode,
} from "../../lib/ipc";
import type { PermissionsView, PermissionRuleKind, PermissionScope } from "../../lib/types";
import "../tools/tools.css";
import "./permissions.css";

/** The three modes, in escalation order, with the copy shown under the select. */
const MODES: { value: string; label: string; blurb: string }[] = [
  {
    value: "relaxed",
    label: "Relaxed",
    blurb:
      "Only recognizably dangerous commands (deleting files, killing processes, rewriting git history) and commands the parser can't see through ask first. Everything else runs.",
  },
  {
    value: "cautious",
    label: "Cautious",
    blurb:
      "Only recognizably read-only commands run unprompted; file writes/edits and git commits ask too. For unfamiliar codebases or high-stakes work.",
  },
  {
    value: "bypass",
    label: "Bypass",
    blurb:
      "Nothing asks — except circuit breakers (rm -rf /, home deletion, .git and shell-rc writes), which always refuse.",
  },
];

const KIND_LABEL: Record<PermissionRuleKind, string> = {
  allow: "Always allow (prefix)",
  allow_exact: "Always allow (exact command)",
  deny: "Always deny (prefix)",
};

export function PermissionsPage() {
  const [view, setView] = useState<PermissionsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [draftKind, setDraftKind] = useState<PermissionRuleKind>("allow");
  const [draftScope, setDraftScope] = useState<PermissionScope>("project");

  const refresh = useCallback(() => {
    getPermissions()
      .then((v) => {
        setView(v);
        setError(null);
      })
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(refresh, [refresh]);

  async function pickMode(mode: string) {
    try {
      await setPermissionMode(mode);
      refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function removeRule(scope: PermissionScope, kind: PermissionRuleKind, value: string) {
    try {
      await removePermissionRule(scope, kind, value);
      refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function addRule() {
    const value = draft.trim();
    if (!value) return;
    try {
      await addPermissionRule(draftScope, draftKind, value);
      setDraft("");
      refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  if (!view) {
    return <div className="tools-page">{error ?? "Loading…"}</div>;
  }
  const active = MODES.find((m) => m.value === view.mode) ?? MODES[0];

  return (
    <div className="tools-page perms-page">
      {error && <div className="perms-error">{error}</div>}

      <section className="perms-section">
        <h3 className="perms-heading">Mode</h3>
        <p className="perms-blurb">
          How eagerly the agent asks before running commands. Applies to new and resumed chats.
        </p>
        <div className="perms-modes">
          {MODES.map((m) => (
            <button
              key={m.value}
              type="button"
              className={`perms-mode ${view.mode === m.value ? "active" : ""}`}
              onClick={() => pickMode(m.value)}
            >
              {m.label}
            </button>
          ))}
        </div>
        <p className="perms-mode-blurb">{active.blurb}</p>
      </section>

      <section className="perms-section">
        <h3 className="perms-heading">Rules</h3>
        <p className="perms-blurb">
          Deny always wins over allow. Prefix rules match on word boundaries (<code>git push</code>{" "}
          matches <code>git push origin</code>, not <code>git pushx</code>) and only apply to
          commands the parser fully understood — obfuscated commands can only match exact rules.
        </p>

        <RuleScope
          title="This project"
          subtitle={view.project_path}
          rules={view.project}
          onRemove={(kind, value) => removeRule("project", kind, value)}
        />
        <RuleScope
          title="Everywhere"
          subtitle="~/.oxen-harness/permissions.json"
          rules={view.global}
          onRemove={(kind, value) => removeRule("global", kind, value)}
        />

        <form
          className="perms-add"
          onSubmit={(e) => {
            e.preventDefault();
            addRule();
          }}
        >
          <select
            className="perms-select"
            value={draftScope}
            onChange={(e) => setDraftScope(e.target.value as PermissionScope)}
            aria-label="Rule scope"
          >
            <option value="project">This project</option>
            <option value="global">Everywhere</option>
          </select>
          <select
            className="perms-select"
            value={draftKind}
            onChange={(e) => setDraftKind(e.target.value as PermissionRuleKind)}
            aria-label="Rule kind"
          >
            <option value="allow">Allow prefix</option>
            <option value="allow_exact">Allow exact</option>
            <option value="deny">Deny prefix</option>
          </select>
          <input
            className="perms-input"
            placeholder="e.g. cargo test — or a full command for exact rules"
            value={draft}
            spellCheck={false}
            onChange={(e) => setDraft(e.target.value)}
          />
          <button type="submit" className="perms-add-btn" disabled={!draft.trim()}>
            Add rule
          </button>
        </form>
      </section>

      <section className="perms-section">
        <h3 className="perms-heading">
          <ShieldAlert size={15} className="perms-shield" /> Always protected
        </h3>
        <p className="perms-blurb">
          Regardless of mode or rules, the agent refuses to run <code>rm -rf /</code> or delete the
          home directory, and never writes to <code>.git</code> internals, shell startup files, or
          its own permission rules. Approved dangerous commands snapshot the workspace first
          (recoverable from <code>~/.oxen-harness/snapshots</code>), and every decision is logged
          to <code>~/.oxen-harness/permissions.jsonl</code>.
        </p>
      </section>
    </div>
  );
}

function RuleScope({
  title,
  subtitle,
  rules,
  onRemove,
}: {
  title: string;
  subtitle: string;
  rules: PermissionsView["global"];
  onRemove: (kind: PermissionRuleKind, value: string) => void;
}) {
  const groups: { kind: PermissionRuleKind; values: string[] }[] = [
    { kind: "deny", values: rules.deny },
    { kind: "allow", values: rules.allow },
    { kind: "allow_exact", values: rules.allow_exact },
  ];
  const empty = groups.every((g) => g.values.length === 0);
  return (
    <div className="perms-scope">
      <div className="perms-scope-head">
        <span className="perms-scope-title">{title}</span>
        <span className="perms-scope-path">{subtitle}</span>
      </div>
      {empty ? (
        <div className="perms-empty">No rules yet — approve a command with “always allow” or add one below.</div>
      ) : (
        groups
          .filter((g) => g.values.length > 0)
          .map((g) => (
            <div className="perms-group" key={g.kind}>
              <span className={`perms-kind ${g.kind}`}>{KIND_LABEL[g.kind]}</span>
              <div className="perms-rules">
                {g.values.map((value) => (
                  <span className="perms-rule" key={value}>
                    <code>{value}</code>
                    <button
                      type="button"
                      className="perms-rule-x"
                      aria-label={`Remove rule ${value}`}
                      onClick={() => onRemove(g.kind, value)}
                    >
                      <X size={12} />
                    </button>
                  </span>
                ))}
              </div>
            </div>
          ))
      )}
    </div>
  );
}
