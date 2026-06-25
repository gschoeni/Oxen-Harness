import { useState } from "react";
import { Button } from "../../components/ui";
import { answerQuestion } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { QuestionAnswer, QuestionPayload } from "../../lib/types";
import "./questions.css";

/** Renders the clarifying-question prompt when one is pending. */
export function QuestionModal() {
  const payload = useStore((s) => s.question);
  if (!payload) return null;
  // Keyed by id so a new question resets the inner form's selection state.
  return <QuestionForm key={payload.id} payload={payload} />;
}

function QuestionForm({ payload }: { payload: QuestionPayload }) {
  const setQuestion = useStore((s) => s.setQuestion);

  // Selected option labels per question; single-select defaults to the first.
  const [sel, setSel] = useState<string[][]>(() =>
    payload.questions.map((q) => (q.multiSelect ? [] : q.options.length ? [q.options[0].label] : [])),
  );
  const [other, setOther] = useState<string[]>(() => payload.questions.map(() => ""));

  function choose(qi: number, label: string, multi: boolean) {
    setSel((prev) => {
      const next = prev.map((a) => [...a]);
      if (multi) {
        const i = next[qi].indexOf(label);
        if (i >= 0) next[qi].splice(i, 1);
        else next[qi].push(label);
      } else {
        next[qi] = [label];
      }
      return next;
    });
  }

  async function submit() {
    const answers: QuestionAnswer[] = payload.questions.map((q, qi) => {
      const selected = [...sel[qi]];
      const free = other[qi].trim();
      if (free) selected.push(free);
      return { header: q.header || "", question: q.question, selected };
    });
    setQuestion(null);
    try {
      await answerQuestion(payload.id, answers);
    } catch {
      /* the question may have been cancelled; nothing to do */
    }
  }

  return (
    <div className="modal-scrim">
      <div className="modal question-modal" role="dialog" aria-modal="true">
        <div className="modal-header">
          <h2 className="modal-title">A quick question</h2>
        </div>
        <form
          className="question-form"
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
        >
          {payload.questions.map((q, qi) => (
            <fieldset className="question-block" key={qi}>
              <legend className="question-legend">
                {q.header && <span className="qchip">{q.header}</span>}
                {q.question}
              </legend>
              {q.options.map((opt) => (
                <label className={`qoption ${sel[qi].includes(opt.label) ? "checked" : ""}`} key={opt.label}>
                  <input
                    type={q.multiSelect ? "checkbox" : "radio"}
                    name={`q${qi}`}
                    checked={sel[qi].includes(opt.label)}
                    onChange={() => choose(qi, opt.label, q.multiSelect)}
                  />
                  <span className="qoption-main">
                    <span className="qoption-label">{opt.label}</span>
                    {opt.description && <span className="qoption-desc">{opt.description}</span>}
                  </span>
                </label>
              ))}
              <label className="qoption qother">
                <span className="qoption-label">Other</span>
                <input
                  type="text"
                  className="qother-text"
                  placeholder="Type your own answer…"
                  value={other[qi]}
                  onChange={(e) =>
                    setOther((prev) => prev.map((v, i) => (i === qi ? e.target.value : v)))
                  }
                />
              </label>
            </fieldset>
          ))}
          <div className="question-foot">
            <span className="muted">Pick an option or type your own.</span>
            <Button type="submit" variant="primary">
              Send answer
            </Button>
          </div>
        </form>
      </div>
    </div>
  );
}
