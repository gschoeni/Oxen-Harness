import { CircleCheckBig, Circle, ListChecks } from "lucide-react";
import { useStore } from "../../lib/store";
import { currentPlan, planProgress } from "../../lib/plan";
import type { PlanItem } from "../../lib/types";
import "./plan.css";

const NO_ITEMS: never[] = [];

/** The pinned, live-updating plan panel shown above the thread. Derives the
 *  current plan from the chat's most recent `update_plan` tool call, so it
 *  updates in place as the agent ticks items off — and works in a resumed chat,
 *  since the plan lives in the transcript. Renders nothing when there's no plan. */
export function Plan() {
  const items = useStore((s) => (s.session ? s.threads[s.session.session_id] : undefined)) ?? NO_ITEMS;
  const plan = currentPlan(items);
  if (!plan) return null;
  const { done, total } = planProgress(plan);

  return (
    <section className="plan-panel" aria-label="Task plan">
      <header className="plan-head">
        <ListChecks size={15} className="plan-head-icon" />
        <span className="plan-title">Plan</span>
        <span className="plan-progress">
          {done}/{total}
        </span>
      </header>
      <PlanChecklist items={plan} />
    </section>
  );
}

/** The checklist rows — shared by the pinned panel and the inline tool card. */
export function PlanChecklist({ items }: { items: PlanItem[] }) {
  return (
    <ul className="plan-list">
      {items.map((it, i) => (
        <li key={i} className={`plan-item ${it.status}`}>
          <span className="plan-mark" aria-hidden>
            {it.status === "completed" ? (
              <CircleCheckBig size={15} />
            ) : it.status === "in_progress" ? (
              <span className="plan-spinner" />
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
