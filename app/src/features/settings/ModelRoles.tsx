import { useEffect, useState } from "react";
import { getModelRoles, setModelRole } from "../../lib/ipc";
import { Select } from "../../components/ui/Select";
import type { CloudModel, ModelRole, ModelRoles } from "../../lib/types";

/** The routed roles, in display order: what each one does and what it falls
 *  back to when no model is assigned. */
const ROLES: { role: ModelRole; label: string; blurb: string; fallback: string }[] = [
  {
    role: "study",
    label: "Study game",
    blurb: "Writes and grades the Trail of Understanding's questions about your codebase.",
    fallback: "the smol model, then the chat's model",
  },
  {
    role: "smol",
    label: "Smol",
    blurb: "Fleet lanes and code-review passes, which read and search far more than they write.",
    fallback: "the chat's model",
  },
  {
    role: "summary",
    label: "Summary",
    blurb: "Compaction summaries, when a long chat is condensed to make room.",
    fallback: "the chat's model",
  },
];

/** Assign cheaper models to the work that doesn't need the chat's model. Each
 *  role is one select over the saved cloud models; "use the fallback" clears
 *  the override. Changes apply to new and resumed chats (the study game reads
 *  its role on the next batch of questions). */
export function ModelRolesSection({ models }: { models: CloudModel[] }) {
  const [roles, setRoles] = useState<ModelRoles | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getModelRoles()
      .then(setRoles)
      .catch((err) => setError(String(err)));
  }, []);

  async function assign(role: ModelRole, model: string) {
    setError(null);
    try {
      setRoles(await setModelRole(role, model || null));
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <section className="settings-section">
      <div className="settings-label">Model roles</div>
      <p className="hint">
        Route side work to a cheaper, faster model. A role with no model uses its fallback.
      </p>
      <div className="model-roles">
        {ROLES.map(({ role, label, blurb, fallback }) => {
          const value = roles?.[role] ?? "";
          // A role assigned from the CLI may name a model that isn't saved
          // here; keep it selectable so the select never shows something else.
          const options = [
            { value: "", label: `Use ${fallback}` },
            ...(value && !models.some((m) => m.id === value) ? [{ value, label: value }] : []),
            ...models.map((m) => ({ value: m.id, label: m.id })),
          ];
          return (
            <div className="media-field" key={role}>
              <span className="media-field-label">{label}</span>
              <Select
                label={label}
                value={value}
                options={options}
                disabled={roles === null}
                onValueChange={(model) => assign(role, model)}
                searchable
              />
              <span className="media-field-hint">{blurb}</span>
            </div>
          );
        })}
      </div>
      {error && <span className="save-status err">{error}</span>}
    </section>
  );
}
