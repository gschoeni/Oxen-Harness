import { CircleCheckBig, Circle, CirclePause } from "lucide-react";
import type { PlanItem } from "../../lib/types";
import "./plan.css";

/** The checklist rows — rendered by the inline tool card.
 *  `live` says whether the agent is actively running: the in-progress row spins
 *  only then, and shows a paused marker otherwise (a stopped run must not spin
 *  forever). */
export function PlanChecklist({ items, live = false }: { items: PlanItem[]; live?: boolean }) {
  return (
    <ul className="plan-list">
      {items.map((it, i) => (
        <li key={i} className={`plan-item ${it.status}`}>
          <span className="plan-mark" aria-hidden>
            {it.status === "completed" ? (
              <CircleCheckBig size={15} />
            ) : it.status === "in_progress" ? (
              live ? (
                <span className="plan-spinner" />
              ) : (
                <CirclePause size={15} />
              )
            ) : (
              <Circle size={15} />
            )}
          </span>
          <span className="plan-text">
            {it.status === "in_progress" ? it.active_form : it.content}
          </span>
        </li>
      ))}
    </ul>
  );
}
