import { useCallback, useEffect, useState } from "react";
import {
  Braces,
  CheckCircle2,
  Code2,
  FlaskConical,
  MessageSquarePlus,
  Pause,
  Play,
  RefreshCw,
  Square,
  Upload,
} from "lucide-react";
import type { ViewProps } from "../../workbench-sdk";
import {
  PackageSurface,
  refreshPackages,
  type ViewPackage,
} from "../../features/workbench/packages";
import "./studio.css";

interface Diagnostic {
  level: string;
  message: string;
  file?: string;
  line?: number;
}
export interface StudioStatus {
  source: string;
  report_path: string;
  active: boolean;
  paused: boolean;
  dirty: boolean;
  mounted: boolean;
  installed_digest?: string | null;
  candidate?: ViewPackage | null;
  package?: ViewPackage | null;
  diagnostics: Diagnostic[];
  runtime: Diagnostic[];
  test?: {
    id: string;
    digest: string;
    status: string;
    results: { name: string; passed: boolean; error?: string }[];
    snapshot: string;
  } | null;
}
export function ViewStudio({ api }: ViewProps) {
  const [source, setSource] = useState(api.context.target.path ?? "views/my-view"),
    [id, setId] = useState("my.view"),
    [title, setTitle] = useState("My view"),
    [status, setStatus] = useState<StudioStatus>(),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false),
    [details, setDetails] = useState(false),
    [resource, setResource] = useState("");
  const request = useCallback(
    (action: string, extra: Record<string, unknown> = {}) =>
      api.request<StudioStatus>("develop", { action, source, ...extra }),
    [api, source],
  );
  useEffect(() => {
    setSource(api.context.target.path ?? "views/my-view");
    setStatus(undefined);
  }, [api.context.target.path]);
  useEffect(() => {
    if (!api.context.target.path) return;
    let live = true;
    void request("status")
      .catch(() => request("check"))
      .then((value) => {
        if (live) setStatus(value);
      })
      .catch((e) => {
        if (live) setError(String(e));
      });
    return () => {
      live = false;
    };
  }, [request, api.context.target.path]);
  useEffect(() => {
    if (!status?.active) return;
    let live = true,
      timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const next = await request("status");
        if (live) setStatus(next);
      } catch (e) {
        if (live) setError(String(e));
      }
      if (live) timer = setTimeout(poll, 1000);
    }
    timer = setTimeout(poll, 1000);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [request, status?.active]);
  useEffect(() => {
    if (status?.installed_digest) void refreshPackages().catch((e) => setError(String(e)));
  }, [status?.installed_digest]);
  async function operate(action: string, extra: Record<string, unknown> = {}) {
    setBusy(true);
    setError("");
    try {
      const result = await request(action, extra);
      if (action === "install") {
        await refreshPackages();
        setStatus(await request("status"));
        setDetails(true);
      } else setStatus(result);
      if (action === "scaffold" || action === "preview")
        api.open({ view: "view-studio", path: source });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  const buildWithAgent = () =>
    api.addToChat(
      `Build my custom work view in ${source}. ${status ? `Read ${source}/AGENTS.md and use the develop_view tool to inspect its status at ${status.report_path}.` : `Use develop_view scaffold with id ${id} and title ${JSON.stringify(title)}.`} Edit the package files, preview without rebuilding the app, run its browser tests, and fix errors. Keep my requested functionality and the current file permissions. Ask what I want this view to do.`,
    );
  const candidate = status?.candidate,
    active = status?.active && status.package;
  const issues = [
    ...(status?.diagnostics ?? []),
    ...(status?.runtime ?? []).filter((d) => d.level === "error" || d.level === "warn"),
  ];
  return (
    <div className={`view-studio${active ? " is-preview" : ""}`}>
      <header className="studio-heading">
        <span className="studio-mark">
          <Braces size={18} />
        </span>
        <div>
          <strong>View Studio</strong>
          <span>Your ideas, part of your workspace.</span>
        </div>
        <button
          className="icon-btn sm"
          title="Build with agent"
          aria-label="Build view with agent"
          onClick={buildWithAgent}
        >
          <MessageSquarePlus size={16} />
        </button>
      </header>
      {!active ? (
        <div className="studio-start">
          <span className="studio-eyebrow">MAKE IT YOURS</span>
          <h2>
            A new view.
            <br />
            Built right here.
          </h2>
          <p>
            Describe a tool to your agent, or start with a working example. Edit its files and see
            the changes without restarting the app.
          </p>
          <label>
            Package folder
            <input
              value={source}
              onChange={(e) => {
                setSource(e.target.value);
                setStatus(undefined);
              }}
              placeholder="views/storyboard"
            />
          </label>
          <div className="studio-fields">
            <label>
              Package ID
              <input
                value={id}
                onChange={(e) => setId(e.target.value)}
                placeholder="my.storyboard"
              />
            </label>
            <label>
              Title
              <input
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                placeholder="Storyboard"
              />
            </label>
          </div>
          <div className="studio-actions">
            <button
              className="studio-primary"
              disabled={busy || !source.trim()}
              onClick={() => void operate("scaffold", { id, title })}
            >
              <Code2 size={15} />
              Create starter
            </button>
            <button disabled={busy || !source.trim()} onClick={() => void operate("check")}>
              <RefreshCw size={15} />
              Open existing
            </button>
          </div>
          <p className="studio-footnote">
            The starter is plain HTML, CSS, and JavaScript, with a saved document and browser tests.
            No build tools required.
          </p>
          {candidate && (
            <section className="studio-review">
              <h3>{candidate.manifest.title}</h3>
              <p>Preview runs this package with these project capabilities:</p>
              <dl>
                {Object.entries(candidate.manifest.permissions).map(([key, paths]) => (
                  <div key={key}>
                    <dt>{key}</dt>
                    <dd>{paths.length ? paths.join(", ") : "None"}</dd>
                  </div>
                ))}
              </dl>
              <button
                className="studio-primary"
                disabled={busy}
                onClick={() => void operate("preview", { digest: candidate.digest })}
              >
                <Play size={15} />
                Start live preview
              </button>
            </section>
          )}
        </div>
      ) : (
        <>
          <div className="studio-toolbar">
            <span title={source}>
              {status.package?.manifest.title}
              <small>
                {status.package?.digest.slice(0, 8)} ·{" "}
                {status.paused ? "Updates paused" : "Watching files"}
              </small>
            </span>
            <button
              title={status.paused ? "Resume updates" : "Pause updates"}
              aria-label={status.paused ? "Resume updates" : "Pause updates"}
              disabled={busy}
              onClick={() => void operate(status.paused ? "resume" : "pause")}
            >
              {status.paused ? <Play size={14} /> : <Pause size={14} />}
            </button>
            <button
              disabled={busy || !status.mounted || status.test?.status === "pending"}
              onClick={() => {
                setDetails(true);
                void operate("test");
              }}
            >
              <FlaskConical size={14} />
              Test
            </button>
            <button
              disabled={
                busy ||
                !candidate ||
                issues.length > 0 ||
                status.test?.status === "pending" ||
                candidate.digest !== status.package?.digest
              }
              onClick={() => void operate("install", { digest: candidate?.digest })}
            >
              <Upload size={14} />
              Install
            </button>
            <button
              title="Stop preview"
              aria-label="Stop preview"
              disabled={busy}
              onClick={() => void operate("stop")}
            >
              <Square size={13} />
            </button>
          </div>
          <div className="studio-preview">
            <PackageSurface
              api={api}
              packageId={status.package!.manifest.id}
              revision={status.package!.digest}
              development
              resourcePath={resource || undefined}
            />
          </div>
        </>
      )}
      {status?.installed_digest && (
        <div className="studio-installed" role="status">
          <CheckCircle2 size={14} />
          Installed {status.installed_digest.slice(0, 8)}
          <button
            onClick={() =>
              api.open({
                view: `package:${status.package?.manifest.id ?? candidate?.manifest.id}`,
                path: resource || undefined,
              })
            }
          >
            Open installed view
          </button>
        </div>
      )}
      {error && (
        <p className="studio-error" role="alert">
          {error}
        </p>
      )}
      {status && (
        <footer className="studio-footer">
          <button onClick={() => setDetails(!details)} aria-expanded={details}>
            {status.test?.status === "passed" ? (
              <CheckCircle2 size={13} />
            ) : (
              <FlaskConical size={13} />
            )}{" "}
            {status.test ? `Tests ${status.test.status}` : "Checks & console"}
            {issues.length ? ` · ${issues.length} issue${issues.length === 1 ? "" : "s"}` : ""}
          </button>
          <button onClick={() => api.open({ view: "editor", path: `${source}/main.js` })}>
            Edit source
          </button>
          <button onClick={buildWithAgent}>Build with agent</button>
        </footer>
      )}
      {details && status && (
        <section className="studio-console" aria-label="View diagnostics">
          <label>
            Preview document (optional)
            <input
              value={resource}
              onChange={(e) => setResource(e.target.value)}
              placeholder="data/my.view/document.json"
            />
          </label>
          <p>
            <code>{status.report_path}</code>
            <button
              onClick={() =>
                api.addToChat(
                  `Inspect and fix my view ${source}. Its current development report is ${status.report_path}.\n${JSON.stringify({ diagnostics: status.diagnostics, runtime: status.runtime.slice(-10), test: status.test }, null, 2)}`,
                )
              }
            >
              Send report to agent
            </button>
          </p>
          {status.test?.results.map((result, index) => (
            <div className={result.passed ? "studio-test-pass" : "studio-error"} key={index}>
              {result.passed ? "✓" : "×"} {result.name}
              {result.error && <pre>{result.error}</pre>}
            </div>
          ))}
          {[...status.diagnostics, ...status.runtime].map((d, index) => (
            <div key={index} className={d.level === "error" ? "studio-error" : "studio-log"}>
              <small>
                {d.level} {d.file}
                {d.line ? `:${d.line}` : ""}
              </small>
              <pre>{d.message}</pre>
            </div>
          ))}
          {!status.diagnostics.length && !status.runtime.length && <p>No runtime messages yet.</p>}
          {active && candidate && status.diagnostics.length > 0 && (
            <button
              disabled={busy}
              onClick={() => void operate("preview", { digest: candidate.digest })}
            >
              Restart preview with current permissions
            </button>
          )}
          {active && candidate && status.diagnostics.length > 0 && (
            <dl className="studio-grants">
              {Object.entries(candidate.manifest.permissions).map(([key, paths]) => (
                <div key={key}>
                  <dt>{key}</dt>
                  <dd>{paths.length ? paths.join(", ") : "None"}</dd>
                </div>
              ))}
            </dl>
          )}
          <p>
            Installing keeps this exact version in the view picker. Continue editing and install
            again when the next version is ready.
          </p>
        </section>
      )}
    </div>
  );
}
