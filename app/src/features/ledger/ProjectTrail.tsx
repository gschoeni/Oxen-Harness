// A project's full trail — every open thread in this workspace with working
// waystations, plus its settled tally. The home board shows only a train's
// first few wagons; "…and N more on this trail" lands here, so this list is
// deliberately uncapped. Rendered on the project's home page.

import { useState } from "react";
import { useBoard } from "./useBoard";
import { SettledRow, WagonRow } from "./Wagon";

export function ProjectTrail({ workspace }: { workspace: string }) {
  const board = useBoard();
  // One unfolded waystation at a time, same as the board.
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [showSettled, setShowSettled] = useState(false);

  const train = board?.trains.find((t) => t.workspace === workspace);
  const settled = board?.settled.filter((t) => t.entry.workspace === workspace) ?? [];
  const lost = board?.lost.filter((t) => t.entry.workspace === workspace) ?? [];
  if (!train && settled.length === 0 && lost.length === 0) return null;

  return (
    <section className="project-trail" aria-label="This project's threads">
      <h2 className="ledger-label">On the trail</h2>
      {train?.threads.map((thread) => (
        <WagonRow
          key={thread.entry.id}
          thread={thread}
          expanded={expandedId === thread.entry.id}
          onToggle={() =>
            setExpandedId((cur) => (cur === thread.entry.id ? null : thread.entry.id))
          }
        />
      ))}
      {!train && <p className="project-trail-quiet">Nothing riding right now.</p>}

      {settled.length > 0 && (
        <div className="project-trail-settled">
          <button
            className="ledger-foot-toggle"
            onClick={() => setShowSettled((v) => !v)}
            aria-expanded={showSettled}
          >
            <span className="ledger-label">Settled here</span>
            <span className="ledger-foot-tally">
              <span className="ledger-foot-count">({settled.length})</span>
            </span>
          </button>
          {showSettled && (
            <div className="ledger-foot-rows">
              {settled.map((thread) => (
                <SettledRow key={thread.entry.id} thread={thread} />
              ))}
            </div>
          )}
        </div>
      )}
      {lost.length > 0 && (
        <p className="project-trail-quiet">
          {lost.length} older thread{lost.length === 1 ? "" : "s"} lost to the trail — see the
          Ledger’s archive.
        </p>
      )}
    </section>
  );
}
