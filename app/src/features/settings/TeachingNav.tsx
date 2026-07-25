// The header shared by Tools, Skills, and Rules — the three ways the agent can
// be taught something.
//
// Each page is legible on its own, but which one you want depends on knowing
// the other two exist: people reach for a skill when they wanted a rule, or
// stuff a prohibition into the system prompt because they never learned rules
// were a thing. So all three pages open with the same trio, the current one
// marked, each a one-line definition and a way to get there.

import { GraduationCap, Radar, Wrench } from "lucide-react";
import type { ReactNode } from "react";
import { useStore } from "../../lib/store";
import type { SettingsPage } from "../../lib/types";
import "./teaching.css";

const WAYS: { page: SettingsPage; icon: ReactNode; label: string; is: string }[] = [
  { page: "tools", icon: <Wrench size={14} />, label: "Tools", is: "what it can do" },
  {
    page: "skills",
    icon: <GraduationCap size={14} />,
    label: "Skills",
    is: "what it knows how to do",
  },
  { page: "rules", icon: <Radar size={14} />, label: "Rules", is: "what it must not do" },
];

export function TeachingNav({ current }: { current: SettingsPage }) {
  const setPage = useStore((s) => s.setSettingsPage);
  return (
    <nav className="teaching-nav" aria-label="Ways to teach the agent">
      {WAYS.map((way) => {
        const here = way.page === current;
        return (
          <button
            key={way.page}
            type="button"
            className={`teaching-way ${here ? "here" : ""}`}
            onClick={() => !here && setPage(way.page)}
            aria-current={here ? "page" : undefined}
          >
            <span className="teaching-way-head">
              {way.icon}
              {way.label}
            </span>
            <span className="teaching-way-is">{way.is}</span>
          </button>
        );
      })}
    </nav>
  );
}
