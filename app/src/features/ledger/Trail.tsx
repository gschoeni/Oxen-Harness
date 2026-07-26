// The trail line — the Ledger's signature mark. One horizontal line per
// thread: solid where the wagon has been, stitched where it hasn't, with the
// plan's tick-marks as stations along the working stretch, camp where the
// last word was spoken, and the settle ring waiting at the end. Everything is
// positioned by percentage from `trailShape`, so the line is honest at any
// width; everything is painted with theme tokens, so every theme re-skins it.

import { CAMP_AT, trailShape, type Thread } from "./ledger";

/** `settling` plays the tie-off ritual: the line pulls taut to the ring and
 *  the knot pops in, ahead of the state actually flipping to settled.
 *  `dust` is a changing key — each change kicks up one puff behind the wagon
 *  (a tool call just started; the keyed span remounts and replays). */
export function Trail({
  thread,
  settling = false,
  dust = 0,
}: {
  thread: Thread;
  settling?: boolean;
  dust?: number;
}) {
  const shape = trailShape(thread);
  const pct = (f: number) => `${(f * 100).toFixed(2)}%`;
  const settled = thread.state === "settled" || settling;
  const progress = settled ? 1 : shape.progress;

  return (
    <div
      className={`trail trail-state-${thread.state} trail-weather-${thread.weather} ${settling ? "trail-settling" : ""}`}
      aria-hidden="true"
    >
      <span className="trail-behind" style={{ width: pct(progress) }} />
      {!settled && <span className="trail-ahead" style={{ left: pct(progress) }} />}
      <span className="trail-post trail-post-start" />
      {shape.ticks.map((at, i) => (
        <span
          key={i}
          className={`trail-tick ${i < shape.ticksDone ? "done" : ""}`}
          style={{ left: pct(at) }}
        />
      ))}
      {shape.stations.map((station) => (
        <span
          key={station.name + station.at}
          className={`trail-station ${station.done ? "done" : ""}`}
          style={{ left: pct(station.at) }}
          title={station.name}
        />
      ))}
      {!settled && <span className="trail-camp" style={{ left: pct(CAMP_AT) }} />}
      <span className="trail-ring">{settled && <span className="trail-ring-knot" />}</span>
      {!settled && <span className="trail-wagon" style={{ left: pct(progress) }} />}
      {dust > 0 && <span key={dust} className="trail-dust" style={{ left: pct(progress) }} />}
    </div>
  );
}
