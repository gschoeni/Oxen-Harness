// The backend the study cabinet talks to. The game queues requests by kind
// (see gameKit's request channel); this maps each kind onto the typed ipc
// wrapper for the chat on screen, so the game itself never knows about
// sessions or Tauri.

import { useMemo } from "react";
import { studyAnswer, studyBatch, studyProfile } from "../../../lib/ipc";
import { useStore } from "../../../lib/store";
import type { StudyAnswerRequest, StudyBatchRequest } from "../../../lib/types";
import type { HeroGameHost } from "./gameKit";
import { STUDY_ANSWER, STUDY_BATCH, STUDY_PROFILE } from "./study";

export function studyHost(session: string | undefined): HeroGameHost {
  return {
    perform(kind, payload) {
      if (!session) return Promise.reject(new Error("Open a chat in a project first."));
      switch (kind) {
        case STUDY_PROFILE:
          return studyProfile(session);
        case STUDY_BATCH:
          return studyBatch(session, payload as StudyBatchRequest);
        case STUDY_ANSWER:
          return studyAnswer(session, payload as StudyAnswerRequest);
        default:
          return Promise.reject(new Error(`unknown game request: ${kind}`));
      }
    },
  };
}

/** The game host for the chat on screen. */
export function useGameHost(): HeroGameHost {
  const session = useStore((s) => s.session?.session_id);
  return useMemo(() => studyHost(session), [session]);
}
