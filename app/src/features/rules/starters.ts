// Rules worth having on day one, and the shape a new one starts from.
//
// Offered when a project has none: a rule is easier to adapt than to invent,
// and each of these is one a real repository would want.

import type { RuleSpec } from "../../lib/types";

/** Rules worth having on day one, offered when there are none. */
export const STARTERS: RuleSpec[] = [
  {
    name: "no-unwrap",
    when: "\\.unwrap\\(\\)",
    scope: ["tool"],
    message:
      "This project doesn't use `.unwrap()` outside tests — return a Result, or use `expect` with a reason that says what invariant is being relied on.",
    interrupt: true,
    repeat: "once",
    enabled: true,
  },
  {
    name: "leave-generated-alone",
    when: "generated/",
    scope: ["tool"],
    message:
      "Files under generated/ are produced by the build. Change the generator or its input instead, then re-run it.",
    interrupt: true,
    repeat: "once",
    enabled: true,
  },
  {
    name: "no-force-push",
    when: "push\\s+--force|push\\s+-f\\b",
    scope: ["tool"],
    message: "Don't force-push. If history needs fixing, say what you'd do and let me decide.",
    interrupt: true,
    repeat: "once",
    enabled: true,
  },
];

export const blank = (): RuleSpec => ({
  name: "",
  when: "",
  scope: ["tool"],
  message: "",
  interrupt: true,
  repeat: "once",
  enabled: true,
});
