import { useEffect, useRef, useState } from "react";

import { hfTokenPresent, resolveHfModel, searchHfModels, setHfToken } from "../../lib/ipc";
import type { CatalogModel, HfHit } from "../../lib/types";

/** Does the input look like a directly-loadable repo (`owner/name`) or HF URL,
 *  rather than a free-text search term? */
export function looksLikeRepo(input: string): boolean {
  const s = input.trim();
  if (s.includes("huggingface.co")) return true;
  return /^[\w.-]+\/[\w.-]+/.test(s);
}

/** The Hugging Face model finder: one smart input that autocompletes GGUF repos
 *  as you type, or resolves a pasted repo / .gguf link directly, plus the
 *  optional access token for gated repos. It owns all of that UI state and the
 *  debounced search; the host supplies `onResolved` (a resolved model to select)
 *  and `onError`, the two things that live outside this widget. */
export function useHfSearch(opts: {
  onResolved: (model: CatalogModel) => void;
  onError: (message: string) => void;
}) {
  const { onResolved, onError } = opts;

  const [input, setInput] = useState("");
  const [results, setResults] = useState<HfHit[]>([]);
  const [searching, setSearching] = useState(false);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [resolving, setResolving] = useState(false);
  const [hasToken, setHasToken] = useState(false);
  const [tokenInput, setTokenInput] = useState("");
  const [showToken, setShowToken] = useState(false);
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    hfTokenPresent().then(setHasToken).catch(() => {});
  }, []);

  // Live autocomplete: debounce the input and search Hugging Face for GGUF repos
  // as the user types. A stale request can't clobber a newer one (cancelled flag).
  useEffect(() => {
    const q = input.trim();
    if (q.length < 2) {
      setResults([]);
      setSearching(false);
      return;
    }
    let cancelled = false;
    setSearching(true);
    const t = setTimeout(() => {
      searchHfModels(q)
        .then((r) => {
          if (cancelled) return;
          setResults(r);
          setOpen(true);
          setActive(-1);
        })
        .catch(() => !cancelled && setResults([]))
        .finally(() => !cancelled && setSearching(false));
    }, 220);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  }, [input]);

  // Close the suggestions dropdown on an outside click.
  useEffect(() => {
    if (!open) return;
    function onDown(e: MouseEvent) {
      if (boxRef.current && !boxRef.current.contains(e.target as Node)) setOpen(false);
    }
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);

  async function resolve(repoOrUrl: string) {
    if (!repoOrUrl.trim()) return;
    setResolving(true);
    setOpen(false);
    onError("");
    try {
      onResolved(await resolveHfModel(repoOrUrl.trim()));
    } catch (e) {
      onError(String(e));
    } finally {
      setResolving(false);
    }
  }

  // Pick the suggestion at index `i` (or the input itself if it's a repo/URL).
  function choose(i: number) {
    if (i >= 0 && i < results.length) resolve(results[i].repo);
    else if (looksLikeRepo(input)) resolve(input);
  }

  function onKeyDown(e: React.KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setOpen(true);
      setActive((a) => Math.min(a + 1, results.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(a - 1, -1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(active);
    } else if (e.key === "Escape") {
      setOpen(false);
    }
  }

  async function saveToken() {
    try {
      await setHfToken(tokenInput.trim());
      setHasToken(!!tokenInput.trim());
      setShowToken(false);
    } catch (e) {
      onError(`Could not save token: ${e}`);
    }
  }

  return {
    input,
    setInput,
    results,
    searching,
    open,
    setOpen,
    active,
    setActive,
    resolving,
    hasToken,
    tokenInput,
    setTokenInput,
    showToken,
    setShowToken,
    boxRef,
    resolve,
    onKeyDown,
    saveToken,
  };
}
