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
  development?: boolean;
  theme?: Record<string, string>;
}
export interface TestContext {
  assert(condition: unknown, message?: string): asserts condition;
}
export interface ViewAPI {
  readonly apiVersion: 1;
  readonly context: ViewContext;
  read(path: string): Promise<DocumentSnapshot>;
  save(path: string, content: string, revision?: string): Promise<DocumentSnapshot>;
  asset(path: string): Promise<string>;
  request<T = unknown>(action: string, payload?: Record<string, unknown>): Promise<T>;
  open(target: ViewTarget): Promise<unknown>;
  addToChat(text: string): Promise<unknown>;
  report(state: Record<string, unknown>): Promise<unknown>;
  subscribeFiles(handler: () => void): () => void;
  watch(
    path: string,
    handler: (snapshot: DocumentSnapshot) => void,
    onError?: (error: unknown) => void,
  ): () => void;
  /** Retain a JSON draft through preview reloads, scoped to this conversation and package. */
  retain(value: unknown): Promise<void>;
  restore<T = unknown>(): Promise<T | null>;
  /** Register a test to run inside the actual preview, on explicit request. */
  test(name: string, run: (context: TestContext) => void | Promise<void>): void;
}
export function getViewAPI(): ViewAPI;
declare global {
  interface Window {
    readonly oxenView: ViewAPI;
  }
}
