import { useEffect, useSyncExternalStore } from "react";
import type { DocumentSnapshot, WorkbenchAPI } from ".";

export interface Draft {
  snapshot?: DocumentSnapshot;
  content: string;
  dirty: boolean;
  saving: boolean;
  error?: string;
  conflict?: DocumentSnapshot;
}
const EMPTY: Draft = { content: "", dirty: false, saving: false };

/** Shared by every renderer of the same project file, including hidden contexts. */
export class DocumentStore {
  private drafts = new Map<string, Draft>();
  private listeners = new Set<() => void>();
  private loading = new Map<string, Promise<void>>();
  key(api: WorkbenchAPI, path: string) {
    return `${api.context.workspace}\0${path}`;
  }
  get(key: string) {
    return this.drafts.get(key) ?? EMPTY;
  }
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  private put(key: string, draft: Draft) {
    this.drafts.set(key, draft);
    this.listeners.forEach((fn) => fn());
  }
  edit(key: string, content: string) {
    const draft = this.get(key);
    this.put(key, { ...draft, content, dirty: content !== draft.snapshot?.content });
  }
  async load(api: WorkbenchAPI, path: string) {
    const key = this.key(api, path);
    const pending = this.loading.get(key);
    if (pending) return pending;
    const work = (async () => {
      try {
        const snapshot = await api.read(path);
        const draft = this.get(key);
        if (draft.dirty || draft.saving) {
          if (snapshot.revision !== draft.snapshot?.revision)
            this.put(key, { ...draft, conflict: snapshot });
        } else this.put(key, { snapshot, content: snapshot.content, dirty: false, saving: false });
      } catch (e) {
        this.put(key, { ...this.get(key), error: String(e) });
      }
    })();
    this.loading.set(key, work);
    try {
      await work;
    } finally {
      this.loading.delete(key);
    }
  }
  async save(api: WorkbenchAPI, path: string) {
    const key = this.key(api, path),
      before = this.get(key);
    if (before.saving) throw new Error("A save is already in progress");
    if (before.conflict)
      throw new Error("File changed on disk. Resolve the conflict before saving.");
    this.put(key, { ...before, saving: true, error: undefined });
    try {
      const snapshot = await api.save(path, before.content, before.snapshot?.revision);
      const latest = this.get(key);
      this.put(key, {
        snapshot,
        content: latest.content,
        dirty: latest.content !== snapshot.content,
        saving: false,
      });
      return snapshot;
    } catch (e) {
      this.put(key, { ...this.get(key), saving: false, error: String(e) });
      await this.load(api, path);
      throw e;
    }
  }
  resolve(key: string, choice: "disk" | "draft") {
    const draft = this.get(key);
    if (!draft.conflict) return;
    const content = choice === "disk" ? draft.conflict.content : draft.content;
    this.put(key, {
      snapshot: draft.conflict,
      content,
      dirty: content !== draft.conflict.content,
      saving: false,
    });
  }
}
export const documents = new DocumentStore();

export function useDocument(api: WorkbenchAPI, path: string) {
  const key = documents.key(api, path);
  const draft = useSyncExternalStore(documents.subscribe, () => documents.get(key));
  useEffect(() => {
    void documents.load(api, path);
    return api.subscribeFiles(() => {
      void documents.load(api, path);
    });
  }, [api, path]);
  return {
    ...draft,
    edit: (content: string) => documents.edit(key, content),
    save: () => documents.save(api, path),
    reload: () => documents.load(api, path),
    resolve: (choice: "disk" | "draft") => documents.resolve(key, choice),
  };
}
