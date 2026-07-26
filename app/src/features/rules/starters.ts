// The shape a new rule starts from.
//
// The rules worth *suggesting* live in `harness_runtime::rules::suggestions`
// and arrive over `list_rule_suggestions`, so the desktop gallery and
// `/rules suggest` offer the same set, described the same way and carrying the
// same examples. A second copy here would drift out of agreement with it.

import type { RuleSpec } from "../../lib/types";

export const blank = (): RuleSpec => ({
  name: "",
  when: "",
  scope: ["tool"],
  message: "",
  interrupt: true,
  repeat: "once",
  enabled: true,
});
