// The one place a chat's tab color comes from.
//
// A tab wears one of five states, and every surface that paints a chat's
// standing (the strip, the history rows, the history badge) asks here rather
// than reading run flags itself — so "what does the orange dot mean" has one
// answer everywhere.

import { needLabel, type Thread } from "../threads/threads";
import type { RunStatus } from "../../lib/types";
import type { ApprovalRequestEvent, QuestionPayload } from "../../lib/types";

/** idle → nothing owed · running → a turn in flight · needs → the agent is
 *  parked on the user · check → it finished while they weren't looking ·
 *  broken → the reply never arrived. */
export type TabState = "idle" | "running" | "needs" | "check" | "broken";

export interface TabStatus {
  state: TabState;
  /** Why, in a few words: the tab's tooltip, the history row's subline.
   *  Null when idle. */
  label: string | null;
}

/** Everything the verdict is made from, gathered by [`tabSignals`]. */
export interface TabSignals {
  /** A turn is streaming right now. */
  running: boolean;
  /** It finished offscreen this run and nobody has looked yet. */
  unread: boolean;
  /** A gated tool call is waiting on a decision. */
  approval: boolean;
  /** The model asked a question and is waiting on the answer. */
  question: boolean;
  /** The thread verdict, when the snapshot has met this chat. */
  thread: Thread | null | undefined;
}

/** Precedence, strongest first: parked on the user (approval, question), then
 *  live work, then a lost reply, then a finish to look at.
 *  Live signals outrank the snapshot — it can lag a turn that has
 *  since resumed, and a streaming reply is never "dangling". */
export function tabStatus(x: TabSignals): TabStatus {
  if (x.approval || x.thread?.stuck) return { state: "needs", label: "waiting on your approval" };
  if (x.question) return { state: "needs", label: "asked you a question" };
  if (x.running || x.thread?.state === "running") return { state: "running", label: "Running" };
  if (x.thread?.need === "dangling") return { state: "broken", label: needLabel(x.thread) };
  if (x.unread || x.thread?.need === "unseen") {
    return { state: "check", label: "finished while you were away" };
  }
  return { state: "idle", label: null };
}

/** The slices of store state a verdict reads. */
export interface TabStore {
  runStatus: Record<string, RunStatus | undefined>;
  approvals: Record<string, ApprovalRequestEvent | undefined>;
  question: QuestionPayload | null;
}

/** Gather one chat's signals from the store and the board. */
export function tabSignals(s: TabStore, id: string, thread: Thread | null | undefined): TabSignals {
  return {
    running: s.runStatus[id] === "running",
    unread: s.runStatus[id] === "unread",
    approval: s.approvals[id] !== undefined,
    question: s.question?.session === id,
    thread,
  };
}

/** Whether a chat has a claim on the user: the states the history badge
 *  counts for chats that aren't open as tabs. */
export function wantsUser(status: TabStatus): boolean {
  return status.state === "needs" || status.state === "broken" || status.state === "check";
}
