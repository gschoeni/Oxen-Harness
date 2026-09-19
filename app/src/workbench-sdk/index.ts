/** Public contract for a work view. Modules must not import the app store or IPC. */
import type { ComponentType } from "react";

export const VIEW_API_VERSION = 1;
export interface DocumentSnapshot {
  path: string;
  content: string;
  revision: string;
}
export interface ViewTarget {
  view: string;
  path?: string;
  url?: string;
  paths?: string[];
  id?: string;
}
export interface ViewContext {
  session: string;
  workspace: string;
  target: ViewTarget;
}
export interface WorkbenchAPI {
  context: ViewContext;
  read(path: string): Promise<DocumentSnapshot>;
  save(path: string, content: string, revision?: string): Promise<DocumentSnapshot>;
  request<T>(action: string, payload?: Record<string, unknown>): Promise<T>;
  subscribeFiles(handler: () => void): () => void;
  asset(path: string): Promise<string>;
  open(target: ViewTarget): void;
  report(state: Record<string, unknown>): void;
  addToChat(text: string): void;
}
export interface ViewProps {
  api: WorkbenchAPI;
}
export interface ViewModule {
  id: string;
  title: string;
  description: string;
  /** Higher priority wins; an explicit user choice always wins over matching. */
  priority?: number;
  matches?: (path: string) => boolean;
  component: ComponentType<ViewProps>;
}
export { useDocument, documents } from "./documents";
