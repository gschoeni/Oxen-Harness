// Rules worth having, described so you can decide without reading a regex.
//
// The first version of this was three chips showing raw patterns
// (`\.unwrap\(\)`) with a slug underneath — which tells you nothing unless you
// already know what rules are, and the people looking at this screen are
// exactly the people who don't. Each suggestion now leads with what it does,
// says why in one line, names what it catches in prose, and shows its effect;
// the pattern is available but secondary.

import { useEffect, useState } from "react";
import { Plus, Zap } from "lucide-react";
import { listRuleSuggestions } from "../../lib/ipc";
import type { RuleSpec, RuleSuggestion } from "../../lib/types";

export function RuleSuggestions({
  taken,
  onAdd,
}: {
  /** Names already in use, so a rule can't be added twice. */
  taken: string[];
  onAdd: (rule: RuleSpec) => void;
}) {
  const [all, setAll] = useState<RuleSuggestion[] | null>(null);

  useEffect(() => {
    listRuleSuggestions()
      .then(setAll)
      .catch(() => setAll([]));
  }, []);

  if (!all || all.length === 0) return null;
  const groups = [...new Set(all.map((s) => s.group))];

  return (
    <div className="rule-suggestions">
      {groups.map((group) => (
        <section key={group} className="rule-suggestion-group">
          <h4 className="rule-suggestion-group-name">{group}</h4>
          <div className="rule-suggestion-list">
            {all
              .filter((s) => s.group === group)
              .map((s) => (
                <Suggested
                  key={s.rule.name}
                  suggestion={s}
                  added={taken.includes(s.rule.name)}
                  onAdd={() => onAdd(s.rule)}
                />
              ))}
          </div>
        </section>
      ))}
    </div>
  );
}

function Suggested({
  suggestion,
  added,
  onAdd,
}: {
  suggestion: RuleSuggestion;
  added: boolean;
  onAdd: () => void;
}) {
  const { title, why, catches, rule } = suggestion;
  return (
    <div className={`rule-suggestion ${rule.interrupt ? "interrupts" : "reminds"}`}>
      {/* The title is a sentence, so it stays in the body face — the display
          face is for short labels and wraps these into ribbons. */}
      <h5 className="rule-suggestion-title">{title}</h5>
      <p className="rule-suggestion-why">{why}</p>
      <p className="rule-suggestion-catches">
        Catches <span>{catches}</span>
      </p>
      <div className="rule-suggestion-foot">
        {rule.interrupt ? (
          <span className="rule-effect interrupts">
            <Zap size={11} /> interrupts
          </span>
        ) : (
          <span className="rule-effect">reminds</span>
        )}
        <button
          type="button"
          className="rule-suggestion-add"
          onClick={onAdd}
          disabled={added}
          // Every card's button reads "Add"; the name is what tells a screen
          // reader (and a test) which rule this one adds.
          aria-label={added ? `${rule.name} already added` : `Add ${rule.name}`}
        >
          {added ? (
            "Added"
          ) : (
            <>
              <Plus size={13} /> Add
            </>
          )}
        </button>
      </div>
    </div>
  );
}
