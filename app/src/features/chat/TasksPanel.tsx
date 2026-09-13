// Background shell commands of this chat: one row per task — what it is,
// how long it has run, the last line it printed — with a stop. Appears only
// while there is something to show, and follows the backend's list events
// so it is never stale. Finished tasks fade once the model has read them:
// the registry drops them, and so does this list.

import { useEffect } from "react";
import { Terminal, X } from "lucide-react";
import { useStore } from "../../lib/store";
import type { TaskSummary } from "../../lib/types";

export function TasksPanel() {
  const sessionId = useStore((s) => s.session?.session_id);
  const tasks = useStore((s) => (s.session ? s.tasks[s.session.session_id] : undefined));
  const refreshTasks = useStore((s) => s.refreshTasks);
  const killTask = useStore((s) => s.killTask);
  useEffect(() => {
    if (sessionId) void refreshTasks(sessionId);
  }, [sessionId, refreshTasks]);
  if (!sessionId || !tasks || tasks.length === 0) return null;
  const running = tasks.filter((t) => t.running).length;
  return (
    <div className="fleet-panel tasks-panel" role="status" aria-label="Background tasks">
      <div className="fleet-panel-head">
        <Terminal size={13} className="fleet-panel-icon" />
        <span className="fleet-panel-title">
          {running > 0
            ? `${running} background command${running === 1 ? "" : "s"} running`
            : "Background commands"}
        </span>
        <span className="fleet-panel-hint">output is delivered to the chat when one finishes</span>
      </div>
      <div className="fleet-lanes">
        {tasks.map((task) => (
          <div key={task.id} className="fleet-lane-row">
            <div className="fleet-lane tasks-row" title={task.command}>
              <TaskGlyph task={task} />
              <span className="fleet-lane-name tasks-command">{task.command}</span>
              <span className="fleet-lane-activity">{task.last_line}</span>
              <span className="fleet-lane-tokens">{elapsed(task.elapsed_secs)}</span>
            </div>
            {task.running && (
              <button
                type="button"
                className="fleet-lane-stop"
                onClick={() => killTask(sessionId, task.id)}
                title="Stop this command"
                aria-label={`Stop ${task.command}`}
              >
                <X size={11} />
              </button>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

function TaskGlyph({ task }: { task: TaskSummary }) {
  if (task.running) return <span className="fleet-glyph running" aria-label="running" />;
  const failed = task.killed || (task.exit_code ?? 0) !== 0;
  return (
    <span className={`fleet-glyph ${failed ? "failed" : "done"}`} aria-label={failed ? "ended" : "done"}>
      {failed ? "×" : "✓"}
    </span>
  );
}

function elapsed(secs: number): string {
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  return m < 60 ? `${m}m ${secs % 60}s` : `${Math.floor(m / 60)}h ${m % 60}m`;
}
