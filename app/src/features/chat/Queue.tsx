// Messages stacked while the agent is busy. Each goes to the model at the
// turn's next step (between its tool rounds), in order; "Send now" interrupts
// the turn so they go at once.
// Styled as one of the in-flight panels (fleet, tasks, media) so the stack
// above the composer reads as one column: same width, head, and row height.
import { ListOrdered, X } from "lucide-react";
import { Button } from "../../components/ui";

export function Queue({
  items,
  onChange,
  onSendNow,
}: {
  items: string[];
  onChange: (next: string[]) => void;
  /** Interrupt the running turn so the queue sends immediately; omitted when
   *  nothing is running to interrupt. */
  onSendNow?: () => void;
}) {
  if (items.length === 0) return null;

  return (
    <div className="fleet-panel queue" role="status" aria-label="Queued messages">
      <div className="fleet-panel-head">
        <ListOrdered size={13} className="fleet-panel-icon" />
        <span className="fleet-panel-title">Queued · {items.length}</span>
        <span className="fleet-panel-hint queue-note">sends at the agent's next step</span>
        {onSendNow && (
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="queue-send-now"
            title="Interrupt the current step and send the queue now"
            onClick={onSendNow}
          >
            Send now
          </Button>
        )}
        <Button type="button" size="sm" variant="outline" className="queue-clear" onClick={() => onChange([])}>
          Clear
        </Button>
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
