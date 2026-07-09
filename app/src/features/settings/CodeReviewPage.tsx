// The Code Review settings subpage: edit the pipeline `/code-review` (CLI) and
// the composer's Review button (desktop) both run. The pipeline is an ordered
// list of steps, each a prompt sent to a fresh, isolated agent; each step's
// output feeds the next via {{previous}}, and the last step's reply is parsed
// as the findings report. Saved to ~/.oxen-harness/code-review.json — shared
// with the CLI — and applied to the next review.

import { useEffect, useState } from "react";
import { ArrowDown, ArrowUp, Plus, Split, Trash2 } from "lucide-react";
import { Button } from "../../components/ui";
import {
  defaultCodeReviewConfig,
  getCodeReviewConfig,
  saveCodeReviewConfig,
} from "../../lib/ipc";
import type { CodeReviewConfig, CodeReviewStep } from "../../lib/types";
import "../tools/tools.css";

export function CodeReviewPage() {
  // null until the persisted pipeline loads, so the form never flashes empty.
  const [config, setConfig] = useState<CodeReviewConfig | null>(null);
  const [dirty, setDirty] = useState(false);
  const [status, setStatus] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    getCodeReviewConfig()
      .then(setConfig)
      .catch((e) => setStatus({ ok: false, text: String(e) }));
  }, []);

  function update(next: CodeReviewConfig) {
    setConfig(next);
    setDirty(true);
    setStatus(null);
  }

  function updateStep(i: number, patch: Partial<CodeReviewStep>) {
    if (!config) return;
    update({
      ...config,
      steps: config.steps.map((s, j) => (j === i ? { ...s, ...patch } : s)),
    });
  }

  /** Turn a single-prompt step into a two-agent fan-out (its prompt seeds
   *  the first agent). */
  function splitStep(i: number) {
    if (!config) return;
    const step = config.steps[i];
    updateStep(i, {
      prompt: "",
      agents: [
        { name: `${step.name}-1`, prompt: step.prompt },
        { name: `${step.name}-2`, prompt: step.prompt },
      ],
    });
  }

  function updateAgent(i: number, a: number, patch: Partial<{ name: string; prompt: string }>) {
    if (!config) return;
    const agents = (config.steps[i].agents ?? []).map((ag, j) =>
      j === a ? { ...ag, ...patch } : ag,
    );
    updateStep(i, { agents });
  }

  function addAgent(i: number) {
    if (!config) return;
    const agents = config.steps[i].agents ?? [];
    updateStep(i, {
      agents: [...agents, { name: `agent-${agents.length + 1}`, prompt: "" }],
    });
  }

  /** Remove an agent; dropping to one folds the step back to a single prompt. */
  function removeAgent(i: number, a: number) {
    if (!config) return;
    const agents = (config.steps[i].agents ?? []).filter((_, j) => j !== a);
    if (agents.length === 1) {
      updateStep(i, { prompt: agents[0].prompt, agents: [] });
    } else {
      updateStep(i, { agents });
    }
  }

  function moveStep(i: number, delta: number) {
    if (!config) return;
    const j = i + delta;
    if (j < 0 || j >= config.steps.length) return;
    const steps = [...config.steps];
    [steps[i], steps[j]] = [steps[j], steps[i]];
    update({ ...config, steps });
  }

  function removeStep(i: number) {
    if (!config) return;
    update({ ...config, steps: config.steps.filter((_, j) => j !== i) });
  }

  function addStep() {
    if (!config) return;
    update({
      ...config,
      steps: [
        ...config.steps,
        {
          name: `step-${config.steps.length + 1}`,
          prompt:
            "Continue the review.\n\nTARGET: {{target}}\n\nPREVIOUS STEP'S OUTPUT:\n{{previous}}\n",
        },
      ],
    });
  }

  async function save() {
    if (!config) return;
    // An emptied list silently reads back as the defaults; block it so saving
    // always round-trips to what the form shows.
    if (config.steps.length === 0) {
      setStatus({ ok: false, text: "Add at least one step (or reset to defaults)." });
      return;
    }
    const incomplete = (s: CodeReviewStep) =>
      !s.name.trim() ||
      (s.agents?.length
        ? s.agents.some((a) => !a.name.trim() || !a.prompt.trim())
        : !s.prompt.trim());
    if (config.steps.some(incomplete)) {
      setStatus({
        ok: false,
        text: "Every step (and every parallel agent) needs a name and a prompt.",
      });
      return;
    }
    try {
      await saveCodeReviewConfig(config);
      setDirty(false);
      setStatus({ ok: true, text: "Saved — applies to the next review." });
    } catch (e) {
      setStatus({ ok: false, text: String(e) });
    }
  }

  async function resetToDefaults() {
    try {
      update(await defaultCodeReviewConfig());
      setStatus({ ok: true, text: "Defaults loaded — Save to apply." });
    } catch (e) {
      setStatus({ ok: false, text: String(e) });
    }
  }

  if (!config) {
    return (
      <div className="settings-page">
        {status && <span className="save-status err">{status.text}</span>}
      </div>
    );
  }

  return (
    <div className="settings-page">
      <section className="settings-section">
        <div className="settings-label">Review pipeline</div>
        <p className="hint">
          A code review runs these steps in order — each on a <em>fresh, isolated agent</em> with
          the full tool set, so the verifier judges the code rather than the finder's reasoning.
          The default pipeline is <strong>find</strong> (recall-biased, told not to self-censor) →{" "}
          <strong>verify</strong> (adversarial, CONFIRMED / PLAUSIBLE / REFUTED with quoted
          evidence) → <strong>report</strong> (drop refuted, dedup, rank, cap). Prompts may use{" "}
          <code>{"{{target}}"}</code> (what's being reviewed), <code>{"{{diff}}"}</code> (the
          change), <code>{"{{previous}}"}</code> (the prior step's output), and{" "}
          <code>{"{{max_findings}}"}</code>. The CLI's <code>/code-review</code> runs the same
          pipeline.
        </p>

        {config.steps.map((step, i) => {
          // Only 2+ agents is a fan-out (the backend canonicalizes a lone
          // agent to a plain prompt on load), matching the runtime's is_fan_out.
          const fanOut = (step.agents?.length ?? 0) > 1;
          return (
            <div className="settings-section" key={i}>
              <div className="settings-label">
                Step {i + 1} of {config.steps.length}
                {fanOut ? ` — ${step.agents!.length} parallel agents` : ""}
              </div>
              <label className="field">
                <span className="field-name">Name</span>
                <input
                  className="tool-input mono"
                  value={step.name}
                  onChange={(e) => updateStep(i, { name: e.target.value })}
                  aria-label={`Step ${i + 1} name`}
                />
              </label>
              {!fanOut && (
                <label className="field">
                  <span className="field-name">Prompt</span>
                  <textarea
                    className="tool-input mono"
                    rows={10}
                    value={step.prompt}
                    onChange={(e) => updateStep(i, { prompt: e.target.value })}
                    aria-label={`Step ${i + 1} prompt`}
                  />
                </label>
              )}
              {fanOut &&
                step.agents!.map((agent, a) => (
                  <div className="field" key={a}>
                    <span className="field-name">Agent {a + 1}</span>
                    <div style={{ flex: 1, display: "flex", flexDirection: "column", gap: 6 }}>
                      <input
                        className="tool-input mono"
                        value={agent.name}
                        onChange={(e) => updateAgent(i, a, { name: e.target.value })}
                        aria-label={`Step ${i + 1} agent ${a + 1} name`}
                      />
                      <textarea
                        className="tool-input mono"
                        rows={7}
                        value={agent.prompt}
                        onChange={(e) => updateAgent(i, a, { prompt: e.target.value })}
                        aria-label={`Step ${i + 1} agent ${a + 1} prompt`}
                      />
                      <div className="tool-row-actions">
                        <Button variant="ghost" size="sm" onClick={() => removeAgent(i, a)}>
                          <Trash2 size={14} /> Remove agent
                        </Button>
                      </div>
                    </div>
                  </div>
                ))}
              <div className="tool-row-actions tool-editor-actions">
                {fanOut ? (
                  <Button size="sm" onClick={() => addAgent(i)}>
                    <Plus size={14} /> Add parallel agent
                  </Button>
                ) : (
                  <Button
                    size="sm"
                    onClick={() => splitStep(i)}
                    title="Run this step as several agents at once, each with its own prompt"
                  >
                    <Split size={14} /> Split into parallel agents
                  </Button>
                )}
                <Button size="sm" onClick={() => moveStep(i, -1)} disabled={i === 0}>
                  <ArrowUp size={14} /> Move up
                </Button>
                <Button
                  size="sm"
                  onClick={() => moveStep(i, 1)}
                  disabled={i === config.steps.length - 1}
                >
                  <ArrowDown size={14} /> Move down
                </Button>
                <Button variant="danger" size="sm" onClick={() => removeStep(i)}>
                  <Trash2 size={14} /> Remove step
                </Button>
              </div>
            </div>
          );
        })}

        <div className="tool-row-actions tool-editor-actions">
          <Button size="sm" onClick={addStep}>
            <Plus size={14} /> Add a step
          </Button>
        </div>
      </section>

      <section className="settings-section">
        <div className="settings-label">Report</div>
        <label className="field">
          <span className="field-name">Max findings</span>
          <input
            className="tool-input"
            type="number"
            min={1}
            max={32}
            value={config.max_findings}
            onChange={(e) =>
              update({ ...config, max_findings: Math.max(1, Number(e.target.value) || 1) })
            }
            aria-label="Maximum findings in the final report"
          />
        </label>
        <p className="hint">
          The final report keeps at most this many findings, most severe first. Substituted into
          prompts as <code>{"{{max_findings}}"}</code>.
        </p>
        <label className="field">
          <span className="field-name">Parallel agents at once</span>
          <input
            className="tool-input"
            type="number"
            min={1}
            max={6}
            value={config.max_parallel}
            onChange={(e) =>
              update({
                ...config,
                max_parallel: Math.min(6, Math.max(1, Number(e.target.value) || 1)),
              })
            }
            aria-label="How many parallel agents run at once in a fan-out step"
          />
        </label>
        <p className="hint">
          A fan-out step runs all its agents, at most this many concurrently. The chat shows each
          one as a live lane — click a lane to watch that agent work.
        </p>
      </section>

      <section className="settings-section">
        {status && (
          <span className={`save-status ${status.ok ? "" : "err"}`}>{status.text}</span>
        )}
        <div className="tool-row-actions tool-editor-actions">
          <Button variant="primary" size="sm" onClick={save} disabled={!dirty}>
            Save
          </Button>
          <Button variant="ghost" size="sm" onClick={resetToDefaults}>
            Reset to defaults
          </Button>
        </div>
      </section>
    </div>
  );
}
