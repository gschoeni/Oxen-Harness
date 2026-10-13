// The memory readout in the title bar, beside the running count, shown while
// a local model (chat or image) runs and for a few minutes after a memory notice: how much
// of the machine is in use, how much of that the local models hold, and how
// hard macOS is squeezing — so a laptop that is about to swap says so before
// it stutters. Click for the breakdown.

import { MemoryStick } from "lucide-react";
import { useMenuState } from "../../components/ui/Menu";
import { gb, localFootprint, memoryActive, pressureLabel, useMemory } from "../../lib/memory";
import type { MemoryStatus } from "../../lib/types";

export function MemoryMeter() {
  const status = useMemory((s) => s.status);
  const { open, setOpen, ref } = useMenuState();
  if (!status?.system || !memoryActive(status)) return null;
  const system = status.system;
  const local = localFootprint(status);
  const usedPct = Math.min(100, (system.used_bytes / system.total_bytes) * 100);
  const localPct = Math.min(usedPct, (local / system.total_bytes) * 100);
  return (
    <span className="memory-meter-wrap" ref={ref}>
      <button
        type="button"
        className="memory-meter"
        data-pressure={system.pressure}
        aria-expanded={open}
        aria-label={`Memory: ${gb(system.used_bytes)} of ${gb(system.total_bytes)} GB used, local models ${gb(local)} GB, pressure ${pressureLabel(system.pressure)}`}
        title="Memory in use — click for the breakdown"
        onClick={() => setOpen((o) => !o)}
      >
        <MemoryStick size={12} strokeWidth={2.25} aria-hidden className="memory-meter-icon" />
        <span className="memory-meter-bar" aria-hidden>
          <span className="memory-meter-used" style={{ width: `${usedPct}%` }} />
          <span className="memory-meter-local" style={{ width: `${localPct}%` }} />
        </span>
        <span className="memory-meter-figure">
          {gb(system.used_bytes)}/{gb(system.total_bytes)} GB
        </span>
        {status.processes.length > 0 && <span className="memory-meter-local-figure">local {gb(local)} GB</span>}
      </button>
      {open && <MemoryDetails status={status} />}
    </span>
  );
}

function MemoryDetails({ status }: { status: MemoryStatus }) {
  const system = status.system!;
  const rows: [string, string][] = [
    ["In use", `${gb(system.used_bytes)} of ${gb(system.total_bytes)} GB`],
    ["Free now", `${gb(system.available_bytes)} GB`],
    ["Compressed", `${gb(system.compressed_bytes)} GB`],
    ["Swap", `${gb(system.swap_used_bytes)} GB`],
  ];
  return (
    <div className="menu memory-details" role="dialog" aria-label="Memory details">
      <div className="memory-details-head">
        <span>Memory</span>
        <span className="memory-pressure" data-pressure={system.pressure}>
          {pressureLabel(system.pressure)} pressure
        </span>
      </div>
      <dl className="memory-details-facts">
        {rows.map(([k, v]) => (
          <div key={k}>
            <dt>{k}</dt>
            <dd>{v}</dd>
          </div>
        ))}
      </dl>
      {status.processes.length > 0 && (
        <>
          <div className="memory-details-sub">Local models</div>
          <ul className="memory-details-procs">
            {status.processes.map((p) => (
              <li key={p.pid}>
                <span className="memory-proc-kind">{p.kind}</span>
                <span className="memory-proc-model">{p.model || "unnamed"}</span>
                <span className="memory-proc-size">{gb(p.footprint_bytes)} GB</span>
              </li>
            ))}
          </ul>
        </>
      )}
      {status.notice && (
        <p className={`memory-details-notice ${status.notice.level}`}>{status.notice.message}</p>
      )}
      <p className="memory-details-hint">
        A local model that pushes memory too far is stopped automatically before your apps start to
        swap. Image models map their weights from disk, so most of their size never counts here.
      </p>
    </div>
  );
}
