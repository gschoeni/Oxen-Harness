/** The shared enable/disable switch used by tool and skill rows. */
export function ToolSwitch({
  name,
  enabled,
  onToggle,
}: {
  name: string;
  enabled: boolean;
  onToggle: (name: string, enabled: boolean) => void;
}) {
  return (
    <label
      className="tool-switch"
      title={enabled ? "Enabled — click to turn off" : "Disabled — click to turn on"}
      onClick={(e) => e.stopPropagation()}
    >
      <input
        type="checkbox"
        checked={enabled}
        onChange={(e) => onToggle(name, e.target.checked)}
        aria-label={`${enabled ? "Disable" : "Enable"} ${name}`}
      />
      <span className="tool-switch-track" />
    </label>
  );
}
