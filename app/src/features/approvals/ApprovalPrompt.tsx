import { useState } from "react";
import { ShieldAlert } from "lucide-react";
import { answerApproval } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { ApprovalChoice, ApprovalRequestEvent } from "../../lib/types";
import "./approvals.css";

/** The permission gate's approval card: a dangerous (or mode-gated) tool call
 *  is paused until the user decides. Pinned above the composer like the
 *  clarifying-question card, scoped to the visible chat. */
export function ApprovalPrompt() {
  const request = useStore((s) => (s.session ? s.approvals[s.session.session_id] : undefined));
  if (!request) return null;
  // Keyed by id so a new request resets the free-text draft.
  return <ApprovalCard key={request.id} request={request} />;
}

const ACTION_LABEL: Record<ApprovalRequestEvent["kind"], string> = {
  shell: "wants to run",
  file_edit: "wants to write",
  git_commit: "wants to commit",
  task_kill: "wants to kill a background task",
};

function ApprovalCard({ request }: { request: ApprovalRequestEvent }) {
  const clearApproval = useStore((s) => s.clearApproval);
  const [reason, setReason] = useState("");

  function decide(decision: ApprovalChoice, message?: string) {
    clearApproval(request.session);
    answerApproval(request.id, decision, message).catch(() => {
      /* the request may have been cancelled; nothing to do */
    });
  }

  return (
    <div className="aprompt">
      <div className="aprompt-card">
        <div className="aprompt-head">
          <span className="achip">
            <ShieldAlert size={12} />
            approval
          </span>
          <span className="aprompt-title">
            The agent {ACTION_LABEL[request.kind]}
            {request.risk === "dangerous" && <span className="arisk">dangerous</span>}
          </span>
        </div>

        <pre className="aprompt-command">{request.command}</pre>
        {request.reasons.length > 0 && (
          <div className="aprompt-reasons">Flagged: {request.reasons.join("; ")}.</div>
        )}

        <div className="aprompt-actions">
          <button type="button" className="abtn primary" onClick={() => decide("once")}>
            Run once
          </button>
          <button
            type="button"
            className="abtn"
            title={`Don't ask again this session for ${request.grant_label}`}
            onClick={() => decide("session")}
          >
            Allow for session
          </button>
          {request.offer_project_grant && (
            <button
              type="button"
              className="abtn"
              title={`Don't ask again in this project for ${request.grant_label} (saved to .oxen-harness/permissions.json)`}
              onClick={() => decide("project")}
            >
              Allow for project
            </button>
          )}
          {request.offer_trash && (
            <button
              type="button"
              className="abtn"
              title="Relocate the files into ~/.oxen-harness/trash (kept 7 days) instead of deleting"
              onClick={() => decide("trash")}
            >
              Move to trash instead
            </button>
          )}
          <button type="button" className="abtn deny" onClick={() => decide("deny")}>
            Deny
          </button>
          <button
            type="button"
            className="abtn bypass"
            title="Switch this session to bypass mode — nothing asks again (hard limits like rm -rf / still refuse). Session-only; new chats return to your configured mode."
            onClick={() => decide("bypass")}
          >
            Dangerously allow everything
          </button>
        </div>

        <form
          className="aprompt-reason"
          onSubmit={(e) => {
            e.preventDefault();
            if (reason.trim()) decide("deny", reason.trim());
          }}
        >
          <input
            className="aprompt-reason-input"
            placeholder="Or deny with a reason the model will read…"
            value={reason}
            spellCheck={false}
            onChange={(e) => setReason(e.target.value)}
          />
          <button type="submit" className="abtn deny sm" disabled={!reason.trim()}>
            Deny with reason
          </button>
        </form>
      </div>
    </div>
  );
}
