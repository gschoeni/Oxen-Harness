// Messages stacked while the agent is busy; they send in order as it frees up.
// Styled as one of the in-flight panels (fleet, tasks, media) so the stack
// above the composer reads as one column: same width, head, and row height.
import { ListOrdered, X } from "lucide-react";

export function Queue({
  items,
  onChange,
}: {
  items: string[];
  onChange: (next: string[]) => void;
}) {
  if (items.length === 0) return null;

  return (
    <div className="fleet-panel queue" role="status" aria-label="Queued messages">
      <div className="fleet-panel-head">
        <ListOrdered size={13} className="fleet-panel-icon" />
        <span className="fleet-panel-title">Queued · {items.length}</span>
        <span className="fleet-panel-hint queue-note">sends automatically when the agent is free</span>
        <button type="button" className="fleet-panel-stop queue-clear" onClick={() => onChange([])}>
          Clear
        </button>
      </div>
      <div className="fleet-lanes">
        {items.map((text, i) => (
          <div className="fleet-lane-row" key={i}>
            <div className="fleet-lane queue-item" title={text}>
              <span className="queue-idx">{i + 1}</span>
              <span className="queue-text">{text}</span>
            </div>
            <button
              type="button"
              className="fleet-lane-stop"
              aria-label={`Remove queued message ${i + 1}`}
              onClick={() => onChange(items.filter((_, j) => j !== i))}
            >
              <X size={11} />
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
