import { beforeEach, describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { StudyAnswerRequest, StudyBatchRequest, StudyProfile, StudyQuestion } from "../../../lib/types";
import type { GameRequest, GameResult } from "./gameKit";
import { RUN_LEGS, STUDY_ANSWER, STUDY_BATCH, STUDY_PROFILE, StudyGame as G, type StudyState } from "./study";

const palette = { title: "#f0be8c", primary: "#60b060", secondary: "#aa6e3c", text: "#ece2ce", muted: "#968d7d", danger: "#c94c4c", link: "#f0be8c", background: "#0f1115", surface: "#17191f", border: "#2a2d35" };

function profile(level = 2, understanding = 12, faded = 0): StudyProfile {
  return {
    project: "demo-1234",
    workspace: "/demo",
    understanding,
    level,
    answered: 3,
    territories: [
      { id: "crates/agent", name: "agent", mastery: 0.3, answered: 3, correct: 2, faded },
      { id: "docs", name: "docs", mastery: 0, answered: 0, correct: 0, faded: 0 },
    ],
  };
}

let serial = 0;
function question(kind: StudyQuestion["kind"] = "multiple_choice", difficulty = 1): StudyQuestion {
  serial += 1;
  return {
    id: `q${serial}`,
    territory: "crates/agent",
    kind,
    prompt: `Question ${serial}: what does run_turn do first?`,
    options: kind === "free_text" ? [] : kind === "true_false" ? ["True", "False"] : ["Makes room", "Streams", "Runs tools", "Sleeps"],
    source_path: "crates/agent/src/turn.rs",
    source_lines: [10, 20],
    source_excerpt: "pub async fn run_turn() {\n    make_room();\n}",
    difficulty,
    cached: false,
  };
}

/** A stand-in for the wrapper + backend: answers each queued request once. */
function backend(respond: (request: GameRequest) => GameResult | undefined) {
  let seen = 0;
  const log: GameRequest[] = [];
  return {
    log,
    settle(state: StudyState): StudyState {
      let s = state;
      for (;;) {
        const next = s.requests.find((r) => r.id > seen);
        if (!next) return s;
        seen = next.id;
        log.push(next);
        const result = respond(next);
        if (result) s = G.deliver!(s, next, result);
      }
    },
  };
}

/** A backend that always has questions and grades by a verdict function. */
function happyBackend(verdictFor: (answer: StudyAnswerRequest) => "full" | "partial" | "wrong" = () => "full", kind: StudyQuestion["kind"] = "multiple_choice") {
  return backend((request) => {
    if (request.kind === STUDY_PROFILE) return { ok: true, value: profile() };
    if (request.kind === STUDY_BATCH) {
      const count = (request.payload as StudyBatchRequest).count ?? 5;
      return { ok: true, value: { mode: "expedition", questions: Array.from({ length: count }, () => question(kind)), territory: "agent", tokens_used: 100, model: "m" } };
    }
    if (request.kind === STUDY_ANSWER) {
      const answer = request.payload as StudyAnswerRequest;
      const verdict = verdictFor(answer);
      return {
        ok: true,
        value: {
          grade: { verdict, feedback: verdict === "full" ? "Right." : "No.", correct_answer: "Makes room", explanation: "It makes room in the context first." },
          profile: profile(2, 14),
          tokens_used: 0,
        },
      };
    }
    return undefined;
  });
}

const press = (s: StudyState, ...keys: string[]) => keys.reduce((state, k) => G.handleKey(state, k), s);

describe("Trail of Understanding", () => {
  beforeEach(() => {
    serial = 0;
    window.localStorage.clear();
  });

  it("asks for the profile up front and shows the level on the mode screen", () => {
    const s0 = G.initialState();
    expect(s0.phase).toBe("mode");
    expect(s0.requests.map((r) => r.kind)).toEqual([STUDY_PROFILE]);
    const s = happyBackend().settle(s0);
    expect(s.profile?.level).toBe(2);
    expect(renderToStaticMarkup(G.render(s, palette))).toContain("Level 2");
  });

  it("scouts for a batch when a trail is picked, then asks the first question", () => {
    const be = happyBackend();
    let s = be.settle(G.initialState());
    s = press(s, "1");
    expect(s.phase).toBe("scouting");
    const batch = s.requests[s.requests.length - 1];
    expect(batch.kind).toBe(STUDY_BATCH);
    expect(batch.payload).toMatchObject({ mode: "expedition", exclude: [] });
    s = be.settle(s);
    expect(s.phase).toBe("question");
    expect(s.current?.id).toBe("q1");
    expect(s.label).toBe("agent");
    expect(s.tokens).toBe(100);
    expect(renderToStaticMarkup(G.render(s, palette))).toContain("Question 1: what does run_turn do first?");
  });

  it("sends a choice as its option index and scores a full answer", () => {
    const be = happyBackend();
    let s = be.settle(press(be.settle(G.initialState()), "2"));
    expect(s.mode).toBe("fresh_tracks");
    s = press(s, "9"); // not an option
    expect(s.phase).toBe("question");
    s = press(s, "3");
    expect(s.phase).toBe("grading");
    const sent = s.requests[s.requests.length - 1];
    expect(sent.payload).toEqual({ question_id: "q1", answer: "2", hint_used: false });
    s = be.settle(s);
    expect(s.phase).toBe("result");
    expect(s.score).toBe(100);
    expect(s.streak).toBe(1);
    expect(s.leg).toBe(1);
    expect(renderToStaticMarkup(G.render(s, palette))).toContain("RIGHT!");
    s = press(s, "Enter");
    expect(s.phase).toBe("question");
    expect(s.current?.id).toBe("q2");
  });

  it("opens the hint on H, returns on any key, and halves the points", () => {
    const be = happyBackend();
    let s = be.settle(press(be.settle(G.initialState()), "1"));
    s = press(s, "h");
    expect(s.phase).toBe("hint");
    expect(renderToStaticMarkup(G.render(s, palette))).toContain("make_room();");
    s = press(s, "x");
    expect(s.phase).toBe("question");
    s = be.settle(press(s, "1"));
    expect(be.log[be.log.length - 1].payload).toMatchObject({ hint_used: true });
    expect(s.gained).toBe(50);
  });

  it("collects free-text answers through the text entry", () => {
    const be = happyBackend(() => "partial", "free_text");
    let s = be.settle(press(be.settle(G.initialState()), "1"));
    expect(G.textEntry!(s)).toMatchObject({ label: "Your answer" });
    // Digits don't answer a free-text question.
    expect(press(s, "1").phase).toBe("question");
    expect(G.handleText!(s, " ? ").phase).toBe("hint");
    s = G.handleText!(s, "  it makes room  ");
    expect(s.phase).toBe("grading");
    expect(G.textEntry!(s)).toBeNull();
    expect(s.requests[s.requests.length - 1].payload).toMatchObject({ answer: "it makes room" });
    s = be.settle(s);
    expect(s.grade?.verdict).toBe("partial");
    expect(s.gained).toBe(50);
    expect(s.streak).toBe(0);
    expect(s.missed).toHaveLength(1);
  });

  it("plays eight legs, replays misses at the fort, prefetches, and records the score", () => {
    // Miss the second question; everything else is right.
    const be = happyBackend((a) => (a.question_id === "q2" ? "wrong" : "full"));
    let s = be.settle(press(be.settle(G.initialState()), "1"));
    let fortSeen = false;
    let guard = 0;
    while (s.phase !== "over" && guard++ < 60) {
      if (s.phase === "question") s = be.settle(press(s, "1"));
      else if (s.phase === "message") {
        fortSeen = fortSeen || s.msgTitle === "FORT REVIEW";
        s = press(s, "Enter");
      } else if (s.phase === "result") s = be.settle(press(s, "Enter"));
      else throw new Error(`stuck in ${s.phase}`);
    }
    expect(s.phase).toBe("over");
    expect(fortSeen).toBe(true);
    expect(s.leg).toBe(RUN_LEGS);
    expect(s.correct).toBe(RUN_LEGS - 1);
    // The second batch was requested mid-run and excluded what was already used.
    const batches = be.log.filter((r) => r.kind === STUDY_BATCH);
    expect(batches).toHaveLength(2);
    expect((batches[1].payload as StudyBatchRequest).exclude).toEqual(expect.arrayContaining(["q1", "q2", "q3", "q4"]));
    // 8 legs + 1 fort replay were graded.
    expect(be.log.filter((r) => r.kind === STUDY_ANSWER)).toHaveLength(RUN_LEGS + 1);
    expect(s.score).toBeGreaterThan(700);
    expect(s.best).toBe(s.score);
    expect(window.localStorage.getItem("oxen-hero-best-study")).toBe(String(s.score));
    expect(s.rank).toBeGreaterThan(0);
    const over = renderToStaticMarkup(G.render(s, palette));
    expect(over).toContain("7 of 8 answered in full");
    // Any key starts over on the mode screen, keeping the profile.
    const again = press(s, "Enter");
    expect(again.phase).toBe("mode");
    expect(again.profile).not.toBeNull();
    expect(again.score).toBe(0);
  });

  it("says why a trail can't start and returns to the mode screen", () => {
    const be = backend((r) => (r.kind === STUDY_BATCH ? { ok: false, error: "nothing to study: no uncommitted changes or commits to study" } : r.kind === STUDY_PROFILE ? { ok: true, value: profile() } : undefined));
    let s = be.settle(press(be.settle(G.initialState()), "2"));
    expect(s.phase).toBe("error");
    expect(renderToStaticMarkup(G.render(s, palette))).toContain("no uncommitted changes");
    s = press(s, "Enter");
    expect(s.phase).toBe("mode");
  });

  it("ends the run early when the questions run dry mid-trail", () => {
    let batches = 0;
    const be = backend((r) => {
      if (r.kind === STUDY_PROFILE) return { ok: true, value: profile() };
      if (r.kind === STUDY_BATCH) {
        batches += 1;
        return batches === 1
          ? { ok: true, value: { mode: "review", questions: [question(), question()], territory: "review", tokens_used: 0, model: "" } }
          : { ok: false, error: "nothing to study" };
      }
      return { ok: true, value: { grade: { verdict: "full", feedback: "Right.", correct_answer: "x", explanation: "" }, profile: profile(), tokens_used: 0 } };
    });
    let s = be.settle(press(be.settle(G.initialState()), "4"));
    s = be.settle(press(s, "1"));
    s = be.settle(press(s, "Enter"));
    s = be.settle(press(s, "1"));
    s = be.settle(press(s, "Enter"));
    expect(s.phase).toBe("over");
    expect(s.cutShort).toBe(true);
    expect(renderToStaticMarkup(G.render(s, palette))).toContain("2 of 2 answered in full");
  });

  it("retries an answer the backend failed to grade", () => {
    let fail = true;
    const be = backend((r) => {
      if (r.kind === STUDY_PROFILE) return { ok: true, value: profile() };
      if (r.kind === STUDY_BATCH) return { ok: true, value: { mode: "expedition", questions: [question()], territory: "agent", tokens_used: 0, model: "" } };
      if (fail) {
        fail = false;
        return { ok: false, error: "study model (m): 503" };
      }
      return { ok: true, value: { grade: { verdict: "full", feedback: "Right.", correct_answer: "x", explanation: "" }, profile: profile(), tokens_used: 0 } };
    });
    let s = be.settle(press(be.settle(G.initialState()), "1"));
    s = be.settle(press(s, "1"));
    expect(s.phase).toBe("error");
    s = press(s, "Enter");
    expect(s.phase).toBe("question");
    s = be.settle(press(s, "1"));
    expect(s.phase).toBe("result");
  });

  it("warns once about a fading region before the first question", () => {
    const be = backend((r) => {
      if (r.kind === STUDY_PROFILE) return { ok: true, value: profile(2, 12, 0.6) };
      if (r.kind === STUDY_BATCH) return { ok: true, value: { mode: "expedition", questions: [question(), question()], territory: "agent", tokens_used: 0, model: "" } };
      return { ok: true, value: { grade: { verdict: "full", feedback: "Right.", correct_answer: "x", explanation: "" }, profile: profile(2, 12, 0.6), tokens_used: 0 } };
    });
    let s = be.settle(press(be.settle(G.initialState()), "1"));
    expect(s.phase).toBe("message");
    expect(s.msgTitle).toBe("A FADING TRAIL");
    expect(s.msgLines[0]).toContain("agent");
    s = press(s, "Enter");
    expect(s.phase).toBe("question");
    expect(s.current?.id).toBe("q1");
    s = press(be.settle(press(s, "1")), "Enter");
    expect(s.phase).toBe("question"); // no second warning
  });

  it("announces a level-up on the result card", () => {
    const be = backend((r) => {
      if (r.kind === STUDY_PROFILE) return { ok: true, value: profile(2) };
      if (r.kind === STUDY_BATCH) return { ok: true, value: { mode: "expedition", questions: [question()], territory: "agent", tokens_used: 0, model: "" } };
      return { ok: true, value: { grade: { verdict: "full", feedback: "Right.", correct_answer: "x", explanation: "" }, profile: profile(3, 17), tokens_used: 0 } };
    });
    let s = be.settle(press(be.settle(G.initialState()), "1"));
    s = be.settle(press(s, "1"));
    expect(s.leveledUp).toBe(true);
    expect(renderToStaticMarkup(G.render(s, palette))).toContain("YOU ARE NOW LEVEL 3");
  });

  it("ignores a batch that belongs to an abandoned run", () => {
    const be = happyBackend();
    let s = be.settle(G.initialState());
    const abandoned = press(s, "1");
    const stale = abandoned.requests[abandoned.requests.length - 1];
    // The player backs out and starts again before the first batch lands.
    s = G.onStart!(abandoned);
    s = press(s, "3");
    const late = G.deliver!(s, stale, { ok: true, value: { mode: "expedition", questions: [question()], territory: "old", tokens_used: 0, model: "" } });
    expect(late.phase).toBe("scouting");
    expect(late.queue).toHaveLength(0);
  });

  it("answers by tapping an option", () => {
    const be = happyBackend();
    let s = be.settle(press(be.settle(G.initialState()), "1"));
    // Options start under the one-line prompt; the second sits around y=62/136.
    s = G.handlePointer!(s, { kind: "tap", x: 0.5, y: 62 / 136 });
    expect(s.phase).toBe("grading");
    expect(s.requests[s.requests.length - 1].payload).toMatchObject({ answer: "1" });
  });

  it("draws the attract screen with and without a profile", () => {
    const blank = renderToStaticMarkup(G.renderAttract!(palette, G.initialState()));
    expect(blank).toContain("TRAIL OF UNDERSTANDING");
    expect(blank).toContain("PLAY TO MAP WHAT");
    const loaded = renderToStaticMarkup(G.renderAttract!(palette, happyBackend().settle(G.initialState())));
    expect(loaded).toContain("LEVEL 2");
    expect(loaded).toContain("MASTERY ACROSS 2 REGIONS");
  });
});
