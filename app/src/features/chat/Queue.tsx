// Messages stacked while the agent is busy; they send in order as it frees up.
export function Queue({
  items,
  onChange,
}: {
  items: string[];
  onChange: (next: string[]) => void;
}) {
  if (items.length === 0) return null;

  return (
    <div className="queue">
      <div className="queue-head">
        <span>Queued · {items.length}</span>
        <span className="queue-note">sends automatically when the agent is free</span>
        <button className="queue-clear" onClick={() => onChange([])}>
          Clear
        </button>
      </div>
      {items.map((text, i) => (
        <div className="queue-item" key={i}>
          <span className="queue-idx">{i + 1}</span>
          <span className="queue-text">{text}</span>
          <button
            className="queue-x"
            aria-label="Remove"
            onClick={() => onChange(items.filter((_, j) => j !== i))}
          >
            ✕
          </button>
        </div>
      ))}
    </div>
  );
}
