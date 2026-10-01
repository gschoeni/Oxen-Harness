import { Component, Suspense, useEffect, useMemo, type ReactNode, type PointerEvent } from "react";
import { ArrowLeft, ArrowRight, Pin, Workflow, PanelsTopLeft } from "lucide-react";
import { Select } from "../../components/ui";
import { useStore } from "../../lib/store";
import { workbenchCustomizationEnabled, workflowsEnabled } from "../../lib/features";
import { views, viewById, useViewRegistry, availableTarget } from "./registry";
import { useWorkbenchAPI } from "./api";
import type { ViewTarget } from "../../workbench-sdk";
import "../../modules";
import "./workbench.css";

const EMPTY_TARGET: ViewTarget = { view: "welcome" };

export function Workbench({ onResizeStart }: { onResizeStart?: (e: PointerEvent) => void }) {
  useViewRegistry();
  const session = useStore((s) => s.session);
  const context = useStore((s) => (s.session ? s.workContexts[s.session.session_id] : undefined));
  const target = useMemo(() => availableTarget(context?.current ?? EMPTY_TARGET), [context?.current]);
  if (!session) return null;
  return (
    <WorkbenchContent
      session={session.session_id}
      workspace={session.workspace}
      target={target}
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
  const Icon = module?.icon ?? PanelsTopLeft;
  const customizable = workbenchCustomizationEnabled();
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
        {customizable ? (
          <Select
            className="workbench-view-select"
            label="Work view"
            placeholder="Choose a view"
            value={module ? target.view : ""}
            options={views().map((view) => {
              const Icon = view.icon ?? PanelsTopLeft;
              return { value: view.id, label: view.title, description: view.description, icon: <Icon size={16} /> };
            })}
            onValueChange={(value) => {
              const next = viewById(value);
              api.open({
                view: value,
                path: target.path && next?.matches?.(target.path) ? target.path : undefined,
                paths: target.paths?.filter((path) => next?.matches?.(path)),
              });
            }}
          />
        ) : (
          <span className="workbench-view-label">
            <Icon size={16} aria-hidden="true" />
            {module?.title ?? "Work panel"}
          </span>
        )}
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
              <h2>{customizable ? "Your work, your view" : "Your work"}</h2>
              <p>
                {customizable
                  ? `${workflowsEnabled() ? "Build a workflow, preview" : "Preview"} an app, or open a file. This space follows your conversation.`
                  : "Open a file to get started. This space follows your conversation."}
              </p>
              {customizable && (
                <div className="workbench-choices">
                  {views().map((view) => (
                    <button key={view.id} onClick={() => api.open({ view: view.id })}>
                      <strong>{view.title}</strong>
                      <span>{view.description}</span>
                    </button>
                  ))}
                </div>
              )}
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
