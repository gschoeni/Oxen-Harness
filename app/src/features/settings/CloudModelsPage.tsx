import { Check, ChevronDown, ChevronUp, Cloud, Plus, RefreshCw, Search, Star, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState, type FormEvent } from "react";
import { Button, Modal, Spinner } from "../../components/ui";
import { Markdown } from "../../components/ui/Markdown";
import { compactTokens } from "../../lib/format";
import { addCloudModel, getConnection, removeCloudModel, searchOxenModels } from "../../lib/ipc";
import { formatRate, ratesById } from "../../lib/rates";
import { useStore } from "../../lib/store";
import type { CloudModel, OxenModelHit } from "../../lib/types";

/** Manage the cloud model catalog: browse + search what the configured Oxen
 *  endpoint serves (with per-token pricing and descriptions), add models from
 *  it (or manually by id), remove custom ones, and pick the default. The
 *  default also swaps the current chat, matching the composer picker. */
export function CloudModelsPage() {
  const cloudModels = useStore((s) => s.cloudModels);
  const loadCloudModels = useStore((s) => s.loadCloudModels);
  const changeModel = useStore((s) => s.changeModel);

  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // The endpoint's catalog — fetched once on mount, searched client-side.
  const [host, setHost] = useState("");
  const [catalog, setCatalog] = useState<OxenModelHit[] | null>(null);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);

  useEffect(() => {
    loadCloudModels();
    getConnection()
      .then((c) => setHost(c.host))
      .catch(() => {});
  }, [loadCloudModels]);

  async function loadCatalog() {
    setCatalog(null);
    setCatalogError(null);
    try {
      setCatalog(await searchOxenModels(""));
    } catch (err) {
      setCatalog([]);
      setCatalogError(String(err));
    }
  }
  useEffect(() => {
    loadCatalog();
  }, []);

  // The chat-capable slice of the catalog matching the search box. Endpoints
  // that don't annotate routes list everything rather than nothing.
  const hits = useMemo(() => {
    if (!catalog) return [];
    const routed = catalog.some((h) => h.endpoint !== "");
    const chat = routed ? catalog.filter((h) => h.endpoint === "/chat/completions") : catalog;
    const needle = query.trim().toLowerCase();
    if (!needle) return chat;
    return chat.filter((h) =>
      [h.id, h.name, h.developer, h.summary].some((f) => f.toLowerCase().includes(needle)),
    );
  }, [catalog, query]);

  // Annotate the user's saved models with rates the catalog knows about.
  const rateById = useMemo(() => ratesById(catalog ?? []), [catalog]);

  const savedIds = useMemo(() => new Set(cloudModels.map((m) => m.id)), [cloudModels]);

  async function add(e: FormEvent) {
    e.preventDefault();
    const trimmed = id.trim();
    if (!trimmed) return;
    setBusy(true);
    setError(null);
    try {
      await addCloudModel(trimmed, name.trim());
      await loadCloudModels();
      setId("");
      setName("");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function addHit(hit: OxenModelHit) {
    setError(null);
    try {
      await addCloudModel(hit.id, hit.name);
      await loadCloudModels();
    } catch (err) {
      setError(String(err));
    }
  }

  // The model awaiting removal confirmation (null = no modal open).
  const [pendingRemove, setPendingRemove] = useState<CloudModel | null>(null);
  const [removing, setRemoving] = useState(false);

  async function confirmRemove() {
    if (!pendingRemove) return;
    setRemoving(true);
    setError(null);
    try {
      await removeCloudModel(pendingRemove.id);
      await loadCloudModels();
    } catch (err) {
      setError(String(err));
    } finally {
      setRemoving(false);
      setPendingRemove(null);
    }
  }

  async function makeDefault(modelId: string) {
    try {
      await changeModel(modelId);
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <div className="settings-page">
      <section className="settings-section">
        <div className="settings-label">Your models</div>
        {cloudModels.length === 0 ? (
          <div className="model-empty">
            <Cloud size={26} aria-hidden />
            <div className="model-empty-title">No models yet</div>
            <p className="model-empty-text">
              Pick one from the catalog below — the first model you add becomes the default
              for new chats, and you can switch anytime from the picker beneath the chat box.
            </p>
          </div>
        ) : (
          <div className="model-list">
            {cloudModels.map((m) => (
              <div className={`model-item ${m.selected ? "selected" : ""}`} key={m.id}>
                <button
                  className="model-default"
                  title={m.selected ? "Current default" : "Make default"}
                  aria-label={m.selected ? "Current default model" : "Make default model"}
                  onClick={() => !m.selected && makeDefault(m.id)}
                  disabled={m.selected}
                >
                  <Star size={15} fill={m.selected ? "currentColor" : "none"} />
                </button>
                <div className="model-item-info">
                  <span className="model-item-name">{m.name}</span>
                  <span className="model-item-id">{m.id}</span>
                </div>
                {rateById.has(m.id) && (
                  <span className="model-item-rate">{rateById.get(m.id)}</span>
                )}
                <button
                  className="model-remove"
                  title="Remove model"
                  aria-label={`Remove ${m.name}`}
                  onClick={() => setPendingRemove(m)}
                >
                  <Trash2 size={15} />
                </button>
              </div>
            ))}
          </div>
        )}
        {error && <span className="save-status err">{error}</span>}
        {cloudModels.length > 0 && (
          <p className="hint">
            Switch between models anytime from the picker beneath the chat box — the star
            marks the default for new chats.
          </p>
        )}
      </section>

      <section className="settings-section">
        <div className="settings-label">{host ? `Models on ${host}` : "Endpoint catalog"}</div>
        <div className="catalog-search">
          <Search size={15} aria-hidden />
          <input
            className="field-input"
            type="search"
            placeholder="Search models by name, id, or developer…"
            aria-label="Search hosted models"
            value={query}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>

        {catalog === null && (
          <div className="catalog-status">
            <Spinner /> Loading the model catalog…
          </div>
        )}
        {catalogError && (
          <div className="catalog-status">
            <span className="save-status err">Couldn't load the catalog: {catalogError}</span>
            <Button size="sm" onClick={loadCatalog}>
              <RefreshCw size={14} />
              Retry
            </Button>
          </div>
        )}
        {catalog !== null && !catalogError && hits.length === 0 && (
          <div className="catalog-status">
            {query.trim() ? `No models match “${query.trim()}”.` : "The endpoint lists no models."}
          </div>
        )}

        <div className="catalog-list">
          {hits.map((h) => {
            const added = savedIds.has(h.id);
            const rate = formatRate(h.pricing);
            const open = expanded === h.id;
            return (
              <div className="catalog-item" key={h.id}>
                <div className="catalog-item-row">
                  <div className="catalog-item-info">
                    <span className="catalog-item-head">
                      <span className="model-item-name">{h.name}</span>
                      {h.developer && <span className="catalog-item-dev">{h.developer}</span>}
                    </span>
                    <span className="model-item-id">{h.id}</span>
                    {h.summary && <span className="catalog-item-summary">{h.summary}</span>}
                  </div>
                  <div className="catalog-item-side">
                    {h.context_length != null && (
                      <span
                        className="model-item-rate"
                        title="Context window · max reply size, as reported by the model"
                      >
                        {compactTokens(h.context_length)} ctx
                        {h.max_output_tokens != null
                          ? ` · ${compactTokens(h.max_output_tokens)} out`
                          : ""}
                      </span>
                    )}
                    {rate && <span className="model-item-rate">{rate}</span>}
                    {added ? (
                      <span className="catalog-item-added">
                        <Check size={14} /> Added
                      </span>
                    ) : (
                      <Button size="sm" onClick={() => addHit(h)} aria-label={`Add ${h.name}`}>
                        <Plus size={14} />
                        Add
                      </Button>
                    )}
                    {h.description && (
                      <button
                        className="catalog-item-expand"
                        aria-label={open ? `Hide ${h.name} details` : `Show ${h.name} details`}
                        aria-expanded={open}
                        onClick={() => setExpanded(open ? null : h.id)}
                      >
                        {open ? <ChevronUp size={15} /> : <ChevronDown size={15} />}
                      </button>
                    )}
                  </div>
                </div>
                {open && h.description && (
                  <div className="catalog-item-desc">
                    <Markdown text={h.description} />
                  </div>
                )}
              </div>
            );
          })}
        </div>
        <p className="hint">
          Everything the configured endpoint serves at <code>/chat/completions</code>, with its
          price per million tokens. Change the endpoint on the <strong>Connection</strong> page.
        </p>
      </section>

      <section className="settings-section">
        <div className="settings-label">Add by id</div>
        <form className="model-add" onSubmit={add}>
          <input
            className="field-input"
            placeholder="Model id (e.g. claude-sonnet-4-6)"
            value={id}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            onChange={(e) => setId(e.target.value)}
          />
          <input
            className="field-input"
            placeholder="Display name (optional)"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          <Button variant="primary" size="sm" type="submit" disabled={busy || !id.trim()}>
            <Plus size={15} />
            Add
          </Button>
        </form>
        <p className="hint">
          A fallback for models the catalog doesn't list — anything your endpoint serves
          works by its id.
        </p>
      </section>

      {pendingRemove && (
        <Modal title="Remove model?" onClose={() => !removing && setPendingRemove(null)}>
          <p className="delete-confirm-text">
            Remove <strong>{pendingRemove.name}</strong>{" "}
            {pendingRemove.name !== pendingRemove.id && <code>{pendingRemove.id}</code>} from
            your models?
            {pendingRemove.selected &&
              " It's your current default — the next model in your list takes over."}{" "}
            You can re-add it from the catalog anytime.
          </p>
          <div className="delete-confirm-actions">
            <Button variant="ghost" onClick={() => setPendingRemove(null)} disabled={removing}>
              Cancel
            </Button>
            <Button variant="danger" onClick={confirmRemove} disabled={removing}>
              {removing ? "Removing…" : "Remove"}
            </Button>
          </div>
        </Modal>
      )}
    </div>
  );
}
