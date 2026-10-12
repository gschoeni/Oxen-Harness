import { useEffect, useState } from "react";
import { Button } from "../../components/ui";
import { ChevronDown, Gauge, Zap } from "lucide-react";
import { Menu, MenuHead, MenuItem, useMenuState } from "../../components/ui/Menu";
import { searchOxenModels } from "../../lib/ipc";
import { rateParts } from "../../lib/rates";
import { useStore } from "../../lib/store";
import type { OxenModelHit } from "../../lib/types";
import { ModelRate } from "./ModelPicker";

const NO_TIERS: string[] = [];

/** `ultrafast` → `Ultrafast`: tier ids are the catalog's, the label is ours. */
const tierName = (tier: string) => tier.charAt(0).toUpperCase() + tier.slice(1);

/** A compact dropdown beside the model picker that runs this chat's model on
 *  one of the service tiers its catalog entry prices (e.g. `ultrafast`), or
 *  back on the default. It only appears for a model that offers a tier, and
 *  shows each tier's rates — a faster tier bills every call at its own,
 *  higher price. The choice is this chat's and survives a restart; switching
 *  models starts the next one on its default. */
export function ServiceTierPicker({ disabled }: { disabled: boolean }) {
  const session = useStore((s) => s.session?.session_id);
  const model = useStore((s) => s.session?.model);
  const tiers = useStore((s) => s.session?.service_tiers) ?? NO_TIERS;
  const tier = useStore((s) => s.session?.service_tier ?? null);
  const changeServiceTier = useStore((s) => s.changeServiceTier);

  const { open, setOpen, ref } = useMenuState();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The model's catalog entry, for the rate beside each tier. A failed fetch
  // just means no rates.
  const [hit, setHit] = useState<OxenModelHit | null>(null);

  useEffect(() => {
    if (!open) return;
    setError(null);
    searchOxenModels("")
      .then((listing) => setHit(listing.find((h) => h.id === model) ?? null))
      .catch(() => setHit(null));
  }, [open, model]);

  if (tiers.length === 0) return null;

  async function pick(next: string | null) {
    if (next === tier) {
      setOpen(false);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await changeServiceTier(next);
      setOpen(false);
    } catch (e) {
      // Stay open on a failure: which tier a chat bills at must never be a
      // guess.
      setError(`Couldn't switch speed: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  }

  const rateOf = (id: string | null) => {
    const pricing = id === null ? hit?.pricing : hit?.service_tiers?.find((t) => t.id === id)?.pricing;
    const rate = rateParts(pricing ?? null);
    return rate ? <ModelRate rate={rate} /> : undefined;
  };
  const label = tier ? tierName(tier) : "Standard";
  const Icon = tier ? Zap : Gauge;

  return (
    <div className="picker" ref={ref}>
      <Button
        type="button"
        size="sm"
        variant="ghost"
        className={`picker-btn ${tier ? "picker-accent" : ""}`}
        onClick={() => setOpen((o) => !o)}
        disabled={!session || disabled || busy}
        title={disabled ? "Finish the current turn to switch speed" : `Speed for this chat: ${label}`}
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <Icon size={13} />
        <span className="picker-label">{label}</span>
        <ChevronDown size={13} className="picker-caret" />
      </Button>

      {open && (
        <Menu className="picker-menu">
          <MenuHead aside={hit?.pricing && "$ per 1M tokens"}>Speed for this chat</MenuHead>
          <MenuItem
            active={tier === null}
            name="Standard"
            hint={rateOf(null)}
            onSelect={() => pick(null)}
          />
          {tiers.map((t) => (
            <MenuItem
              key={t}
              active={t === tier}
              icon={<Zap size={13} className="menu-icon" />}
              name={tierName(t)}
              title={`Every call bills at the ${t} rate`}
              hint={rateOf(t)}
              onSelect={() => pick(t)}
            />
          ))}
          {error && (
            <div className="menu-empty menu-error" role="alert">
              {error}
            </div>
          )}
        </Menu>
      )}
    </div>
  );
}
