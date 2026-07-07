import { CircleCheckBig, Circle, CirclePause, ListChecks } from "lucide-react";
import { useStore } from "../../lib/store";
import { currentPlan, planProgress } from "../../lib/plan";
import type { PlanItem } from "../../lib/types";
import "./plan.css";

const NO_ITEMS: never[] = [];

/** The pinned, live-updating plan panel shown above the thread. Derives the
 *  current plan from the chat's most recent `update_plan` tool call, so it
 *  updates in place as the agent ticks items off — and works in a resumed chat,
 *  since the plan lives in the transcript. Renders nothing when there's no plan.
 *  Activity is keyed to the session's run status: once the run ends (finished,
 *  errored, or cancelled) with items unfinished, the panel shows a stalled
 *  state instead of a spinner that never stops. */
export function Plan() {
  const items = useStore((s) => (s.session ? s.threads[s.session.session_id] : undefined)) ?? NO_ITEMS;
  const running = useStore((s) =>
    s.session ? s.runStatus[s.session.session_id] === "running" : false,
  );
  const plan = currentPlan(items);
  if (!plan) return null;
  const { done, total } = planProgress(plan);
  const stalled = !running && done < total;

  return (
    <section className={`plan-panel${stalled ? " stalled" : ""}`} aria-label="Task plan">
      <header className="plan-head">
        <ListChecks size={15} className="plan-head-icon" />
        <span className="plan-title">Plan</span>
        {stalled && (
          <span className="plan-stalled" title="The run ended before this plan finished">
            stalled
          </span>
        )}
        <span className="plan-progress">
          {done}/{total}
        </span>
      </header>
      <PlanChecklist items={plan} live={running} />
    </section>
  );
}

/** The checklist rows — shared by the pinned panel and the inline tool card.
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
