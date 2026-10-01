// The backend the study cabinet talks to. The game queues requests by kind
// (see gameKit's request channel); this maps each kind onto the typed ipc
// wrapper for the chat on screen, so the game itself never knows about
// sessions or Tauri.

import { useMemo } from "react";
import { needsApiKey } from "../../../lib/apiKey";
import { studyAnswer, studyBatch, studyProfile } from "../../../lib/ipc";
import { useStore } from "../../../lib/store";
import type { StudyAnswerRequest, StudyBatchRequest } from "../../../lib/types";
import type { HeroGameHost } from "./gameKit";
import { STUDY_ANSWER, STUDY_BATCH, STUDY_PROFILE } from "./study";

/** What the trail says when the model can't be reached for want of a key —
 *  in place of the provider's raw 401. */
export const STUDY_NEEDS_KEY = "Add an Oxen API key to play: a model writes and grades the questions.";

export function studyHost(session: string | undefined, needsKey = false): HeroGameHost {
  // Only a failure is reworded; the request is still tried, so an endpoint
  // that takes no key keeps working.
  const model = <T,>(call: Promise<T>) =>
    needsKey ? call.catch(() => Promise.reject(new Error(STUDY_NEEDS_KEY))) : call;
  return {
    perform(kind, payload) {
      if (!session) return Promise.reject(new Error("Open a chat in a project first."));
      switch (kind) {
        case STUDY_PROFILE:
          return studyProfile(session);
        case STUDY_BATCH:
          return model(studyBatch(session, payload as StudyBatchRequest));
        case STUDY_ANSWER:
          return model(studyAnswer(session, payload as StudyAnswerRequest));
        default:
          return Promise.reject(new Error(`unknown game request: ${kind}`));
      }
    },
  };
}

/** The game host for the chat on screen. */
export function useGameHost(): HeroGameHost {
  const session = useStore((s) => s.session?.session_id);
  const needsKey = useStore((s) => needsApiKey(s.keyStatus, s.session?.model));
  return useMemo(() => studyHost(session, needsKey), [session, needsKey]);
}
