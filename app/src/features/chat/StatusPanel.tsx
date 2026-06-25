// The trail status panel — the franchise's most recognizable screen element:
// a framed readout of right-aligned "Label: value" rows. Rows come straight from
// the active theme's voice (flavor_top + flavor_bottom), so each theme writes its
// own dashboard (a wagon's date/weather/health, the synthwave grid's vibe, …).

type Row = [string, string];

export function StatusPanel({ rows }: { rows: Row[] }) {
  if (!rows.length) return null;
  return (
    <div className="status-panel">
      <dl className="status-rows">
        {rows.map(([label, value], i) => (
          <div className="status-row" key={`${label}-${i}`}>
            <dt>{label}:</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
    </div>
  );
}
