// A failed model call, from the faint one-line notice in the thread to the
// view behind it. The notice stays quiet (a provider hiccup mid-turn is not an
// alarm), but it is a button: one click opens everything the agent knows about
// the failure — which model and endpoint, the HTTP status, the attempt count,
// what the provider actually sent back, and what the agent did next — so a
// "502: the model provider returned an error" can be debugged from the app
// rather than from a log file.

import { useEffect, useState } from "react";
import { AlertTriangle, Check, ChevronRight, Copy } from "lucide-react";
import { Button, Modal } from "../../components/ui";
import type { ModelErrorDetail } from "../../lib/types";
import "./modelError.css";

/** Where every attempt is also appended, with its request context — mirrors
 *  `harness_config::paths::errors_log`. */
export const ERROR_LOG_PATH = "~/.oxen-harness/errors.jsonl";

/** The retry notice: the same quiet centered line as any system note, but
 *  clickable, with a small "details" affordance so it reads as openable. */
export function ModelErrorNotice({ text, error }: { text: string; error: ModelErrorDetail }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        type="button"
        className="msg notice model-error-notice"
        onClick={() => setOpen(true)}
        title="View the error details"
      >
        <AlertTriangle size={12} aria-hidden className="model-error-notice-icon" />
        <span className="model-error-notice-text">{text}</span>
        <span className="model-error-notice-cta">
          Details
          <ChevronRight size={12} aria-hidden />
        </span>
      </button>
      {open && <ModelErrorModal error={error} onClose={() => setOpen(false)} />}
    </>
  );
}

/** The detail view. `error` is the last failed call the agent reported (absent
 *  when a turn died without a single retry, e.g. a 402); `final` is the turn's
 *  terminal error string when the failure ended the turn. */
export function ModelErrorModal({
  error,
  final,
  onClose,
}: {
  error?: ModelErrorDetail;
  final?: string;
  onClose: () => void;
}) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(t);
  }, [copied]);

  const copy = () => {
    void navigator.clipboard?.writeText(errorReport(error, final)).then(
      () => setCopied(true),
      () => {},
    );
  };

  const body = error?.detail ? prettyDetail(error.detail) : null;

  return (
    <Modal
      title="Model call failed"
      onClose={onClose}
      actions={
        <Button size="sm" variant="ghost" onClick={copy} aria-label="Copy the error report">
          {copied ? <Check size={14} /> : <Copy size={14} />}
          {copied ? "Copied" : "Copy"}
        </Button>
      }
    >
      <div className="model-error">
        {final && (
          <p className="model-error-final">
            <span className="label-caps">Turn ended</span>
            {final}
          </p>
        )}

        {error ? (
          <>
            <dl className="model-error-facts">
              <dt>When</dt>
              <dd>{new Date(error.at).toLocaleTimeString()}</dd>
              {error.model && (
                <>
                  <dt>Model</dt>
                  <dd className="model-error-mono">{error.model}</dd>
                </>
              )}
              {error.endpoint && (
                <>
                  <dt>Endpoint</dt>
                  <dd className="model-error-mono">{error.endpoint}</dd>
                </>
              )}
              {error.status !== undefined && (
                <>
                  <dt>Status</dt>
                  <dd className="model-error-mono">
                    {error.status} {statusName(error.status)}
                  </dd>
                </>
              )}
              {error.attempt !== undefined && (
                <>
                  <dt>Attempt</dt>
                  <dd>
                    {error.attempt}
                    {error.maxAttempts !== undefined && ` of ${error.maxAttempts}`}
                  </dd>
                </>
              )}
              {error.next && (
                <>
                  <dt>Then</dt>
                  <dd>{nextLabel(error.next)}</dd>
                </>
              )}
            </dl>

            <section className="model-error-section">
              <h3 className="label-caps">Error</h3>
              <p className="model-error-message">{error.error}</p>
            </section>

            <section className="model-error-section">
              <h3 className="label-caps">Provider response</h3>
              {body ? (
                <pre className="model-error-body">{body}</pre>
              ) : (
                <p className="muted model-error-empty">
                  No response body came back — the line above is everything the provider sent.
                </p>
              )}
            </section>
          </>
        ) : (
          <p className="muted model-error-empty">
            The turn failed before any call could be retried, so there is no provider response to
            show.
          </p>
        )}

        <p className="model-error-foot">
          Every attempt is also logged with its request context to{" "}
          <code>{ERROR_LOG_PATH}</code>.
        </p>
      </div>
    </Modal>
  );
}

/** The HTTP reason phrase for the statuses providers actually return. */
export function statusName(status: number): string {
  switch (status) {
    case 400:
      return "Bad Request";
    case 401:
      return "Unauthorized";
    case 402:
      return "Payment Required";
    case 403:
      return "Forbidden";
    case 404:
      return "Not Found";
    case 408:
      return "Request Timeout";
    case 413:
      return "Payload Too Large";
    case 422:
      return "Unprocessable";
    case 429:
      return "Too Many Requests";
    case 500:
      return "Internal Server Error";
    case 502:
      return "Bad Gateway";
    case 503:
      return "Service Unavailable";
    case 504:
      return "Gateway Timeout";
    case 529:
      return "Overloaded";
    default:
      return "";
  }
}

function nextLabel(next: NonNullable<ModelErrorDetail["next"]>): string {
  if (next.kind === "switch") return `switched to ${next.model} for this call`;
  const secs = Math.max(1, Math.ceil(next.delayMs / 1000));
  return `retried after ${secs}s`;
}

/** A provider body is usually JSON on one line; re-indent it so the fields
 *  read, and leave anything else (an HTML error page, plain text) as-is. */
export function prettyDetail(detail: string): string {
  try {
    return JSON.stringify(JSON.parse(detail), null, 2);
  } catch {
    return detail;
  }
}

/** A plain-text report of the failure, for pasting into an issue or a chat. */
export function errorReport(error?: ModelErrorDetail, final?: string): string {
  const lines: string[] = [];
  if (final) lines.push(`Turn ended: ${final}`);
  if (error) {
    lines.push(`When: ${new Date(error.at).toISOString()}`);
    if (error.model) lines.push(`Model: ${error.model}`);
    if (error.endpoint) lines.push(`Endpoint: ${error.endpoint}`);
    if (error.status !== undefined) lines.push(`Status: ${error.status} ${statusName(error.status)}`.trim());
    if (error.attempt !== undefined) {
      lines.push(`Attempt: ${error.attempt}${error.maxAttempts !== undefined ? ` of ${error.maxAttempts}` : ""}`);
    }
    if (error.next) lines.push(`Then: ${nextLabel(error.next)}`);
    lines.push(`Error: ${error.error}`);
    if (error.detail) lines.push("", "Provider response:", prettyDetail(error.detail));
  }
  lines.push("", `Log: ${ERROR_LOG_PATH}`);
  return lines.join("\n");
}
