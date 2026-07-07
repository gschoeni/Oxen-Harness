import { useState } from "react";
import { ArrowRight } from "lucide-react";
import { Button } from "../../components/ui";
import { answerQuestion } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { QuestionAnswer, QuestionPayload } from "../../lib/types";
import "./questions.css";

/** The agent's clarifying questions, asked one at a time in a compact card above
 *  the chat input (rather than a blocking, all-at-once modal). */
export function QuestionPrompt() {
  const payload = useStore((s) => s.question);
  if (!payload) return null;
  // Keyed by id so a new batch resets the stepper.
  return <QuestionStepper key={payload.id} payload={payload} />;
}

function QuestionStepper({ payload }: { payload: QuestionPayload }) {
  const setQuestion = useStore((s) => s.setQuestion);
  const total = payload.questions.length;

  const [step, setStep] = useState(0);
  const [answers, setAnswers] = useState<QuestionAnswer[]>([]);
  // The current step's working state.
  const [selected, setSelected] = useState<string[]>([]);
  const [other, setOther] = useState("");

  const q = payload.questions[step];
  const isLast = step === total - 1;

  function send(all: QuestionAnswer[]) {
    setQuestion(null);
    answerQuestion(payload.id, all).catch(() => {
      /* the question may have been cancelled; nothing to do */
    });
  }

  // Record this step's answer (chosen labels + optional free text) and either
  // advance to the next question or submit them all on the last one.
  function commit(labels: string[], free: string) {
    const sel = [...labels];
    const f = free.trim();
    if (f) sel.push(f);
    if (sel.length === 0) return;
    const all = [...answers, { header: q.header || "", question: q.question, selected: sel }];
    if (isLast) {
      send(all);
    } else {
      setAnswers(all);
      setStep(step + 1);
      setSelected([]);
      setOther("");
    }
  }

  // Single-select commits on click; multi-select toggles and waits for Continue.
  function pick(label: string) {
    if (q.multiSelect) {
      setSelected((s) => (s.includes(label) ? s.filter((x) => x !== label) : [...s, label]));
    } else {
      commit([label], "");
    }
  }

  return (
    <div className="qprompt">
      <div className="qprompt-card">
        <div className="qprompt-head">
          {q.header && <span className="qchip">{q.header}</span>}
          <span className="qprompt-question">{q.question}</span>
          {total > 1 && (
            <span className="qprompt-progress" aria-label={`Question ${step + 1} of ${total}`}>
              {step + 1}/{total}
            </span>
          )}
        </div>

        <div className="qprompt-options">
          {q.options.map((opt) => {
            const checked = selected.includes(opt.label);
            return (
              <button
                type="button"
                key={opt.label}
                className={`qopt ${checked ? "checked" : ""}`}
                onClick={() => pick(opt.label)}
              >
                <span className={`qopt-mark ${q.multiSelect ? "box" : "dot"} ${checked ? "on" : ""}`} />
                <span className="qopt-main">
                  <span className="qopt-label">{opt.label}</span>
                  {opt.description && <span className="qopt-desc">{opt.description}</span>}
                </span>
              </button>
            );
          })}
        </div>

        <form
          className="qprompt-other"
          onSubmit={(e) => {
            e.preventDefault();
            commit(q.multiSelect ? selected : [], other);
          }}
        >
          <input
            className="qother-input"
            placeholder="Or type your own answer…"
            value={other}
            spellCheck={false}
            onChange={(e) => setOther(e.target.value)}
          />
          {q.multiSelect ? (
            <Button
              type="submit"
              variant="primary"
              size="sm"
              disabled={selected.length === 0 && !other.trim()}
            >
              {isLast ? "Submit" : "Continue"}
            </Button>
          ) : (
            <button type="submit" className="qother-send" aria-label="Submit answer" disabled={!other.trim()}>
              <ArrowRight size={15} />
            </button>
          )}
        </form>
      </div>
    </div>
  );
}
