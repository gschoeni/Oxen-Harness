/** Public contract for a work view. Modules must not import the app store or IPC. */
import type { ComponentType } from "react";

export const VIEW_API_VERSION = 1;
import type {DocumentSnapshot,ViewTarget,ViewContext} from "../../../packages/view-sdk";
export type {DocumentSnapshot,ViewTarget,ViewContext} from "../../../packages/view-sdk";
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
  /** Small monochrome picker icon. Views without one use a shared fallback. */
  icon?: ComponentType<{ size?: number; className?: string }>;
  /** Higher priority wins; an explicit user choice always wins over matching. */
  priority?: number;
  /** Serializable metadata lets the agent discover and open bundled views. */
  filePatterns?: string[];
  requiresFile?: boolean;
  documentSchema?: Record<string, unknown>;
  agentVisible?: boolean;
  matches?: (path: string) => boolean;
  component: ComponentType<ViewProps>;
}
export { useDocument, documents } from "./documents";
