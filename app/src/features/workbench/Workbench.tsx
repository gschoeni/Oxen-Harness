import { Component, Suspense, useEffect, type ReactNode, type PointerEvent } from "react";
import { ArrowLeft, ArrowRight, Pin, Workflow } from "lucide-react";
import { useStore } from "../../lib/store";
import { workbenchRequest } from "../../lib/ipc";
import { views, viewById, useViewRegistry } from "./registry";
import { useWorkbenchAPI } from "./api";
import type { ViewTarget } from "../../workbench-sdk";
import "../../modules";
import { refreshPackages } from "./packages";
import "./workbench.css";

const EMPTY_TARGET: ViewTarget = { view: "welcome" };

export function Workbench({ onResizeStart }: { onResizeStart?: (e: PointerEvent) => void }) {
  const registered = useViewRegistry();
  const descriptors = JSON.stringify(
    registered
      .filter((view) => view.agentVisible !== false && !view.id.startsWith("package:"))
      .map((view) => ({
        id: view.id,
        title: view.title,
        description: view.description,
        file_patterns: view.filePatterns ?? [],
        requires_file: view.requiresFile ?? false,
        priority: view.priority ?? 0,
        document_schema: view.documentSchema,
      })),
  );
  useEffect(() => {
    void refreshPackages().catch((e) =>
      useStore.getState().addNotice(`Load installed views: ${String(e)}`),
    );
  }, []);
  const session = useStore((s) => s.session);
  const context = useStore((s) => (s.session ? s.workContexts[s.session.session_id] : undefined));
  useEffect(() => {
    if (!session) return;
    void workbenchRequest(session.session_id, "register_views", {
      views: JSON.parse(descriptors),
    }).catch((e) => useStore.getState().addNotice(`Register work views: ${String(e)}`));
  }, [session?.session_id, descriptors]);
  if (!session) return null;
  return (
    <WorkbenchContent
      session={session.session_id}
      workspace={session.workspace}
      target={context?.current ?? EMPTY_TARGET}
      onResizeStart={onResizeStart}
    />
  );
}

function WorkbenchContent({
  session,
  workspace,
  target,
  onResizeStart,
}: {
  session: string;
  workspace: string;
  target: ViewTarget;
  onResizeStart?: (e: PointerEvent) => void;
}) {
  const context = useStore((s) => s.workContexts[session]);
  const api = useWorkbenchAPI({ session, workspace, target });
  const module = viewById(target.view);
  const Module = module?.component;
  useEffect(() => {
    api.report({ status: Module ? "mounted" : "unavailable" });
    return () => api.report({ status: "unmounted" });
  }, [api, Module]);
  return (
    <section className="workbench" aria-label="Current work">
      {onResizeStart && (
        <div
          className="canvas-resizer"
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize work view"
          onPointerDown={onResizeStart}
        />
      )}
      <header className="workbench-head">
        <button
          className="icon-btn sm"
          aria-label="Previous work view"
          disabled={!context || context.cursor === 0}
          onClick={() => useStore.getState().travelWorkView(-1)}
        >
          <ArrowLeft size={14} />
        </button>
        <button
          className="icon-btn sm"
          aria-label="Next work view"
          disabled={!context || context.cursor >= context.history.length - 1}
          onClick={() => useStore.getState().travelWorkView(1)}
        >
          <ArrowRight size={14} />
        </button>
        <select
          aria-label="Work view"
          value={module ? target.view : "welcome"}
          onChange={(e) => {
            const next = viewById(e.target.value);
            api.open({
              view: e.target.value,
              path: target.path && next?.matches?.(target.path) ? target.path : undefined,
              paths: target.paths?.filter((path) => next?.matches?.(path)),
            });
          }}
        >
          <option value="welcome">Choose a view</option>
          {views().map((view) => (
            <option key={view.id} value={view.id}>
              {view.title}
            </option>
          ))}
        </select>
        <span className="workbench-resource" title={target.path ?? target.url}>
          {target.path?.split("/").pop() ?? target.url ?? ""}
        </span>
        <button
          className="icon-btn sm"
          aria-label="Keep this view during agent work"
          aria-pressed={context?.pinned ?? false}
          onClick={() => useStore.getState().pinWorkView()}
          title="Keep this view during agent work"
        >
          <Pin size={14} />
        </button>
      </header>
      <div className="workbench-content">
        <ViewBoundary key={`${session}:${target.view}:${target.path ?? ""}`}>
          {Module ? (
            <Suspense fallback={<p className="workbench-welcome">Loading view…</p>}>
              <Module api={api} />
            </Suspense>
          ) : (
            <div className="workbench-welcome">
              <Workflow size={30} />
              <h2>Your work, your view</h2>
              <p>
                Build a workflow, preview an app, or open a file. This space follows your
                conversation.
              </p>
              <div className="workbench-choices">
                {views().map((view) => (
                  <button key={view.id} onClick={() => api.open({ view: view.id })}>
                    <strong>{view.title}</strong>
                    <span>{view.description}</span>
                  </button>
                ))}
              </div>
              {target.view !== "welcome" && (
                <p role="alert">
                  The view “{target.view}” is unavailable. Your files are still on disk.
                </p>
              )}
            </div>
          )}
        </ViewBoundary>
      </div>
    </section>
  );
}

class ViewBoundary extends Component<{ children: ReactNode }, { error: string | null }> {
  state = { error: null as string | null };
  static getDerivedStateFromError(error: Error) {
    return { error: error.message };
  }
  render() {
    return this.state.error ? (
      <div className="workbench-welcome" role="alert">
        <h2>This view encountered a problem</h2>
        <p>{this.state.error}</p>
        <button onClick={() => this.setState({ error: null })}>Reload view</button>
      </div>
    ) : (
      this.props.children
    );
  }
}
