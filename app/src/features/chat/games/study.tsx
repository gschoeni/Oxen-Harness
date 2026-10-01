// Trail of Understanding — the Oxen Trail's bones, pointed at your own codebase.
// While an agent writes the code, this cabinet quizzes you on it: pick a trail
// (the whole project, the recent changes, what the agent is touching, or what
// you've been forgetting), then answer eight questions a model wrote from the
// real source. Every answer moves a per-project understanding level that fades
// without practice, so the score that matters outlives the run.
//
// Like the other cabinets it is pure state: keys in, state out. It can't call
// the backend, so it queues requests on `state.requests` (profile, a batch of
// questions, an answer to grade) and the wrapper performs them through the
// injected host and hands each outcome back to `deliver` — the same trick the
// games use for sound. Choice questions answer with 1-4; free-text questions
// open the wrapper's text box (`textEntry` / `handleText`).

import { Pine, TitlePlaque, Vista, Wagon } from "./arcadeArt";
import type {
  StudyAnswerRequest,
  StudyAnswerResult,
  StudyBatch,
  StudyBatchRequest,
  StudyGrade,
  StudyMode,
  StudyProfile,
  StudyQuestion,
  ThemePalette,
} from "../../../lib/types";
import {
  clamp,
  COLS,
  GameFrame,
  lastRequestId,
  loadBest,
  poly,
  pushRequest,
  pushSfx,
  Px,
  PxText,
  ROWS,
  saveBest,
  sceneColors,
  type GameRequest,
  type GameResult,
  type HeroGameDefinition,
  type PointerInput,
  type SfxEvent,
  type TextEntry,
} from "./gameKit";
import type { SfxName } from "./sfx";
import { CardFrame, CardTitle, Footer, H, Line, loadTable, ProgressStrip, rankWord, recordScore, screenColors, textCols, W, wrapText, type ScoreEntry, type ScreenColors, type StripMark } from "./terminal";

// Request kinds the cabinet queues; games/studyHost.ts serves them.
export const STUDY_PROFILE = "study.profile";
export const STUDY_BATCH = "study.batch";
export const STUDY_ANSWER = "study.answer";
export const STUDY_FLAG = "study.flag";
export const STUDY_OPEN = "study.open";

/** Questions in one run. */
export const RUN_LEGS = 8;
/** The fort sits here: missed questions come back before the second half. */
const FORT_AT = 4;
const FORT_REPLAYS = 2;
/** Questions fetched per request; the second batch prefetches mid-run. */
const BATCH = 5;
/** A region this far below its peak earns a "fading" trail event. */
const FADING = 0.4;
/** The team you set out with. A wrong answer loses an ox; each one that
    reaches the end is worth a bonus. A carrot, never a wall: the run goes on
    with no oxen left. */
const OXEN = 3;
const OX_BONUS = 50;

type Phase = "mode" | "scouting" | "message" | "question" | "hint" | "grading" | "result" | "over" | "error";
type Tone = "good" | "bad" | "neutral";

export const MODES: { key: StudyMode; name: string; blurb: string }[] = [
  { key: "expedition", name: "Expedition", blurb: "the whole codebase, weakest region first" },
  { key: "fresh_tracks", name: "Fresh tracks", blurb: "what changed lately, from git" },
  { key: "ride_along", name: "Ride-along", blurb: "what your agent just changed" },
  { key: "review", name: "Review", blurb: "missed questions and fading regions" },
];

export interface StudyState {
  phase: Phase;
  clock: number;
  sfx: SfxEvent[];
  requests: GameRequest[];

  // what the backend last said about this project
  profile: StudyProfile | null;
  profileError: string;

  // the run
  mode: StudyMode;
  /** What the current batch is about (a region name, "fresh tracks"…). */
  label: string;
  queue: StudyQuestion[];
  /** Missed questions waiting to be replayed at the fort. */
  fort: StudyQuestion[];
  current: StudyQuestion | null;
  /** The current question is a fort replay: half points, no leg. */
  replay: boolean;
  /** Ids already asked this run, so batches never repeat them. */
  used: string[];
  /** Legs finished (0..RUN_LEGS). */
  leg: number;
  missed: StudyQuestion[];
  fortDone: boolean;
  fadeNoted: boolean;
  hintUsed: boolean;
  /** Request ids being waited on (0 = none). */
  batchPending: number;
  answerPending: number;
  /** Why the last batch failed; only matters if the queue runs dry. */
  batchError: string;
  /** The dry queue already asked once more: a second failure ends the run. */
  batchRetried: boolean;

  // the last answer
  grade: StudyGrade | null;
  gained: number;
  leveledUp: boolean;
  /** The question on the result card was flagged as wrong and struck. */
  flagged: boolean;
  /** An ox was lost to the last answer (shown on its result card). */
  oxLost: boolean;

  // scoring
  score: number;
  oxen: number;
  streak: number;
  bestStreak: number;
  correct: number;
  startLevel: number;
  startUnderstanding: number;
  /** Tokens the study model spent this run. */
  tokens: number;
  best: number;
  newBest: boolean;
  rank: number;
  /** The run ended early because no more questions could be had. */
  cutShort: boolean;

  // message + error cards
  msgTitle: string;
  msgLines: string[];
  msgTone: Tone;
  errorText: string;
  errorBack: Phase;
}

const TABLE_KEY = "study-scores";
const SEED_TABLE: ScoreEntry[] = [
  { name: "Ada Lovelace", score: 1500, occupation: "Expedition" },
  { name: "Grace Hopper", score: 1250, occupation: "Review" },
  { name: "Margaret Hamilton", score: 1000, occupation: "Expedition" },
  { name: "Barbara Liskov", score: 800, occupation: "Fresh tracks" },
  { name: "Donald Knuth", score: 600, occupation: "Ride-along" },
];

// ---- state helpers -----------------------------------------------------------

function fresh(best = loadBest("study"), profile: StudyProfile | null = null, requests: GameRequest[] = []): StudyState {
  return {
    phase: "mode",
    clock: 0,
    sfx: [],
    // Every fresh state asks for the profile, so the attract screen and the
    // mode screen show the level without the player doing anything.
    requests: pushRequest(requests, STUDY_PROFILE),
    profile,
    profileError: "",
    mode: "expedition",
    label: "",
    queue: [],
    fort: [],
    current: null,
    replay: false,
    used: [],
    leg: 0,
    missed: [],
    fortDone: false,
    fadeNoted: false,
    hintUsed: false,
    batchPending: 0,
    answerPending: 0,
    batchError: "",
    batchRetried: false,
    grade: null,
    gained: 0,
    leveledUp: false,
    flagged: false,
    oxLost: false,
    score: 0,
    oxen: OXEN,
    streak: 0,
    bestStreak: 0,
    correct: 0,
    startLevel: profile?.level ?? 1,
    startUnderstanding: profile?.understanding ?? 0,
    tokens: 0,
    best,
    newBest: false,
    rank: 0,
    cutShort: false,
    msgTitle: "",
    msgLines: [],
    msgTone: "neutral",
    errorText: "",
    errorBack: "mode",
  };
}

const cue = (s: StudyState, ...names: SfxName[]): StudyState => ({ ...s, sfx: pushSfx(s.sfx, ...names) });

const isChoice = (q: StudyQuestion | null) => !!q && (q.kind === "multiple_choice" || q.kind === "true_false");
const isOrder = (q: StudyQuestion | null) => q?.kind === "order";

const modeName = (mode: StudyMode) => MODES.find((m) => m.key === mode)?.name ?? "Expedition";

function requestBatch(s: StudyState): StudyState {
  const payload: StudyBatchRequest = {
    mode: s.mode,
    count: BATCH,
    exclude: [...s.used, ...s.queue.map((q) => q.id)],
  };
  const requests = pushRequest(s.requests, STUDY_BATCH, payload);
  return { ...s, requests, batchPending: lastRequestId(requests), batchError: "" };
}

function startRun(s: StudyState, mode: StudyMode): StudyState {
  const run: StudyState = {
    ...fresh(s.best, s.profile, s.requests),
    mode,
    phase: "scouting",
    startLevel: s.profile?.level ?? 1,
    startUnderstanding: s.profile?.understanding ?? 0,
  };
  return cue(requestBatch(run), "menu");
}

function toError(s: StudyState, text: string, back: Phase): StudyState {
  return cue({ ...s, phase: "error", errorText: text, errorBack: back }, "bad");
}

/** The region that has slipped furthest from what the player once knew. */
function fadingRegion(profile: StudyProfile | null) {
  if (!profile) return null;
  const fading = profile.territories.filter((t) => t.answered > 0 && t.faded >= FADING);
  if (fading.length === 0) return null;
  return fading.reduce((a, b) => (b.faded > a.faded ? b : a));
}

/** Legs still to be asked after the one on screen. */
const legsAhead = (s: StudyState) => RUN_LEGS - s.leg - 1;

function nextQuestion(s: StudyState): StudyState {
  // Fort replays come first: they are already in hand.
  if (s.fort.length > 0) {
    const [current, ...fort] = s.fort;
    return { ...s, fort, current, replay: true, phase: "question", hintUsed: false, grade: null, flagged: false, oxLost: false };
  }
  if (s.queue.length === 0) {
    if (s.batchPending) return { ...s, phase: "scouting", current: null };
    if (s.batchError) {
      // A run that never started says why. One underway asks once more — a
      // single 429 on the prefetch shouldn't end it — then ends where it
      // stands, with the reason on the last screen.
      if (s.leg === 0) return toError(s, s.batchError, "mode");
      if (!s.batchRetried) return { ...requestBatch(s), batchRetried: true, phase: "scouting", current: null };
      return finish({ ...s, cutShort: true });
    }
    return { ...requestBatch(s), phase: "scouting", current: null };
  }
  const [current, ...queue] = s.queue;
  let next: StudyState = {
    ...s,
    queue,
    current,
    replay: false,
    used: [...s.used, current.id],
    phase: "question",
    hintUsed: false,
    grade: null,
    flagged: false,
    oxLost: false,
  };
  // Keep a question in hand so the player never waits between legs.
  if (!next.batchPending && !next.batchError && next.queue.length < Math.min(2, legsAhead(next))) {
    next = requestBatch(next);
  }
  // The trail's weather report: once a run, name the region that is fading.
  if (!next.fadeNoted) {
    next = { ...next, fadeNoted: true };
    const region = fadingRegion(next.profile);
    if (region && next.mode !== "review") {
      return {
        ...next,
        phase: "message",
        msgTitle: "A FADING TRAIL",
        msgLines: [
          `Your memory of ${region.name} is slipping:`,
          `${Math.round(region.faded * 100)}% of what you knew has faded.`,
          "A Review run will bring it back.",
        ],
        msgTone: "neutral",
      };
    }
  }
  return next;
}

function submit(s: StudyState, answer: string): StudyState {
  if (!s.current) return s;
  const payload: StudyAnswerRequest = { question_id: s.current.id, answer, hint_used: s.hintUsed };
  const requests = pushRequest(s.requests, STUDY_ANSWER, payload);
  return cue({ ...s, phase: "grading", requests, answerPending: lastRequestId(requests) }, "tick");
}

function applyGrade(s: StudyState, result: StudyAnswerResult): StudyState {
  const q = s.current;
  if (!q) return s;
  const verdict = result.grade.verdict;
  const full = verdict === "full";
  const base = 100 * clamp(q.difficulty || 1, 1, 3);
  let points = full ? base : verdict === "partial" ? base / 2 : 0;
  if (s.hintUsed) points /= 2;
  if (s.replay) points /= 2;
  const streak = full ? s.streak + 1 : 0;
  const bonus = full ? Math.min(50, 10 * (streak - 1)) : 0;
  const gained = Math.round(points) + bonus;
  const before = s.profile?.level ?? s.startLevel;
  const leveledUp = result.profile.level > before;
  const oxLost = verdict === "wrong" && !s.replay && s.oxen > 0;
  const next: StudyState = {
    ...s,
    phase: "result",
    answerPending: 0,
    grade: result.grade,
    gained,
    leveledUp,
    oxLost,
    oxen: s.oxen - (oxLost ? 1 : 0),
    profile: result.profile,
    score: s.score + gained,
    streak,
    bestStreak: Math.max(s.bestStreak, streak),
    correct: s.correct + (full && !s.replay ? 1 : 0),
    tokens: s.tokens + (result.tokens_used || 0),
    leg: s.leg + (s.replay ? 0 : 1),
    missed: !full && !s.replay ? [...s.missed, q] : s.missed,
  };
  if (leveledUp) return cue(next, "good", "milestone");
  return cue(next, full ? "good" : verdict === "partial" ? "tick" : "bad");
}

function finish(s: StudyState): StudyState {
  const score = s.score + s.oxen * OX_BONUS;
  const best = saveBest("study", score);
  const newBest = s.best > 0 ? score > s.best : false;
  const rank = recordScore(TABLE_KEY, SEED_TABLE, score, modeName(s.mode));
  return cue({ ...s, phase: "over", current: null, score, best, newBest, rank }, newBest ? "best" : "good");
}

/** "This question is wrong": retire it everywhere and take back what it
    did to this run — its points, its miss, the ox it cost. */
function flag(s: StudyState): StudyState {
  const q = s.current;
  if (!q || s.flagged) return s;
  const requests = pushRequest(s.requests, STUDY_FLAG, { question_id: q.id });
  return cue(
    {
      ...s,
      requests,
      flagged: true,
      score: s.score - s.gained,
      gained: 0,
      oxen: s.oxen + (s.oxLost ? 1 : 0),
      oxLost: false,
      correct: s.correct - (s.grade?.verdict === "full" && !s.replay ? 1 : 0),
      missed: s.missed.filter((m) => m.id !== q.id),
      fort: s.fort.filter((m) => m.id !== q.id),
      queue: s.queue.filter((m) => m.id !== q.id),
    },
    "tick",
  );
}

/** Open the file the question on screen was about in the editor pane. */
function openSource(s: StudyState): StudyState {
  if (!s.current) return s;
  return { ...s, requests: pushRequest(s.requests, STUDY_OPEN, { path: s.current.source_path }) };
}

/** Give up on a question whose answer couldn't be graded: the leg counts,
    nothing is scored or recorded, and the streak ends. */
function skip(s: StudyState): StudyState {
  return advance({ ...s, leg: s.leg + (s.replay ? 0 : 1), streak: 0, grade: null, current: null });
}

/** After a result card: the fort, the end of the trail, or the next leg. */
function advance(s: StudyState): StudyState {
  if (!s.fortDone && s.leg === FORT_AT && !s.replay) {
    const fort = s.missed.slice(0, FORT_REPLAYS);
    if (fort.length > 0) {
      return {
        ...s,
        fortDone: true,
        fort,
        phase: "message",
        msgTitle: "FORT REVIEW",
        msgLines: [
          "You rest at the fort and go back over",
          fort.length === 1 ? "the question you missed." : `${fort.length} questions you missed.`,
          "Half the points, all of the learning.",
        ],
        msgTone: "neutral",
      };
    }
    return nextQuestion({ ...s, fortDone: true });
  }
  if (s.leg >= RUN_LEGS && s.fort.length === 0) return finish(s);
  return nextQuestion(s);
}

// ---- input -------------------------------------------------------------------

function studyKey(s: StudyState, key: string): StudyState {
  switch (s.phase) {
    case "mode": {
      const n = Number(key);
      if (n >= 1 && n <= MODES.length) return startRun(s, MODES[n - 1].key);
      return s;
    }
    case "message":
      // A message either precedes the question in hand or announces the fort.
      return s.current && !s.grade ? { ...s, phase: "question" } : nextQuestion(s);
    case "question": {
      if (key === "h" || key === "H" || key === "?") return { ...s, phase: "hint", hintUsed: true };
      if (!isChoice(s.current)) return s;
      const n = Number(key);
      if (n >= 1 && n <= (s.current?.options.length ?? 0)) return submit(s, String(n - 1));
      return s;
    }
    case "hint":
      if (key === "o" || key === "O") return openSource(s);
      return { ...s, phase: "question" };
    case "result":
      if (key === "f" || key === "F") return flag(s);
      if (key === "o" || key === "O") return openSource(s);
      return advance(s);
    case "over":
      return cue({ ...fresh(s.best, s.profile, s.requests) }, "menu");
    case "error":
      if (s.errorBack === "mode") return { ...fresh(s.best, s.profile, s.requests) };
      // A grade that keeps failing must not trap the run on one question.
      if (key === "s" || key === "S") return skip(s);
      return { ...s, phase: s.errorBack };
    default:
      return s; // scouting, grading: waiting on the backend
  }
}

function studyText(s: StudyState, text: string): StudyState {
  if (s.phase !== "question" || isChoice(s.current)) return s;
  if (text.trim() === "?") return { ...s, phase: "hint", hintUsed: true };
  if (isOrder(s.current)) {
    // An ordering is the option numbers in sequence; anything else typed
    // around them (spaces, commas, arrows) is ignored. Wait for a full one.
    const digits = text.replace(/\D/g, "");
    return digits.length === s.current!.options.length ? submit(s, digits) : s;
  }
  return submit(s, text.trim());
}

function studyEntry(s: StudyState): TextEntry | null {
  if (s.phase !== "question" || isChoice(s.current)) return null;
  const placeholder = isOrder(s.current)
    ? `type the order, e.g. ${s.current!.options.map((_, i) => i + 1).reverse().join("")} — or ? for a hint`
    : "type your answer — or ? for a hint";
  return { label: "Your answer", placeholder, key: s.current?.id ?? "" };
}

function studyDeliver(s: StudyState, request: GameRequest, result: GameResult): StudyState {
  if (request.kind === STUDY_PROFILE) {
    if (!result.ok) return { ...s, profileError: result.error };
    const profile = result.value as StudyProfile;
    // Until a run starts, the baseline follows the freshest profile.
    // …and a run that began before any profile arrived adopts the first one.
    const idle = s.phase === "mode" || s.profile === null;
    return {
      ...s,
      profile,
      profileError: "",
      startLevel: idle ? profile.level : s.startLevel,
      startUnderstanding: idle ? profile.understanding : s.startUnderstanding,
    };
  }
  if (request.kind === STUDY_BATCH) {
    if (request.id !== s.batchPending) return s; // a previous run's batch
    if (!result.ok) {
      const failed = { ...s, batchPending: 0, batchError: result.error };
      return s.phase === "scouting" ? nextQuestion(failed) : failed;
    }
    const batch = result.value as StudyBatch;
    const known = new Set([...s.used, ...s.queue.map((q) => q.id)]);
    const queue = [...s.queue, ...batch.questions.filter((q) => !known.has(q.id))];
    const next: StudyState = {
      ...s,
      queue,
      label: batch.territory || s.label,
      tokens: s.tokens + (batch.tokens_used || 0),
      batchPending: 0,
      // A batch with nothing new means the well is dry for this mode.
      batchError: queue.length === s.queue.length ? "No more questions could be written for this trail." : "",
      // Asking again for a trail with nothing new would only spend tokens.
      batchRetried: queue.length === s.queue.length ? true : s.batchRetried,
    };
    return s.phase === "scouting" ? nextQuestion(next) : next;
  }
  if (request.kind === STUDY_FLAG) {
    // The record without the struck answers. A failure leaves the question
    // in the bank; the run's own bookkeeping already let it go.
    return result.ok ? { ...s, profile: result.value as StudyProfile } : s;
  }
  if (request.kind === STUDY_ANSWER) {
    if (request.id !== s.answerPending) return s;
    if (!result.ok) return toError({ ...s, answerPending: 0 }, result.error, "question");
    return applyGrade(s, result.value as StudyAnswerResult);
  }
  return s;
}

// ---- layout shared by rendering and taps ---------------------------------------

// Text sizes, in viewBox units. The screen is as wide as the composer, so
// these land around 20px (body) and 17px (small) on screen.
const BODY = 8;
const SMALL = 6.5;
const PROMPT_TOP = 41;
const LINE_STEP = 9;
const MODE_TOP = 52;
const MODE_STEP = 15;
/** Lines of body text that fit between the header and the footer. */
const QUESTION_LINES = 9;
const TEXT_W = W - 20;

/** Where a question's prompt and options sit. Long options get two lines
    unless the whole question would overflow the screen. Columns come from
    the live font, so a narrow face fills the width instead of wrapping early. */
function layoutQuestion(q: StudyQuestion) {
  const cols = textCols(TEXT_W, BODY);
  const prompt = wrapText(q.prompt, cols, 4);
  let options = q.options.map((o) => wrapText(o, cols - 4, 2));
  if (prompt.length + options.reduce((n, o) => n + o.length, 0) > QUESTION_LINES) {
    options = q.options.map((o) => wrapText(o, cols - 4, 1));
  }
  let y = PROMPT_TOP + prompt.length * LINE_STEP + 3;
  const rows = options.map((lines) => {
    const row = { y, lines };
    y += lines.length * LINE_STEP + 1;
    return row;
  });
  return { prompt, rows, end: y };
}

function studyPointer(s: StudyState, p: PointerInput): StudyState {
  if (p.kind !== "tap") return s;
  const y = p.y * H;
  if (s.phase === "mode") {
    const i = Math.round((y - MODE_TOP + 3) / MODE_STEP);
    return i >= 0 && i < MODES.length ? studyKey(s, String(i + 1)) : s;
  }
  if (s.phase === "question" && s.current && isChoice(s.current)) {
    const { rows } = layoutQuestion(s.current);
    const i = rows.findIndex((row) => y >= row.y - LINE_STEP && y < row.y + row.lines.length * LINE_STEP - 4);
    return i >= 0 ? studyKey(s, String(i + 1)) : s;
  }
  return studyKey(s, "Enter");
}

// ---- rendering ---------------------------------------------------------------

const STRIP_MARKS: StripMark[] = Array.from({ length: RUN_LEGS / 2 }, (_, i) => {
  const leg = (i + 1) * 2;
  const type: StripMark["type"] = leg === RUN_LEGS ? "end" : leg === FORT_AT ? "fort" : "flag";
  return { frac: leg / RUN_LEGS, type, key: `leg${leg}` };
});

const pct = (n: number) => `${Math.round(n)}%`;

function levelLine(profile: StudyProfile | null) {
  return profile ? `LV ${profile.level} · ${pct(profile.understanding)}` : "LV —";
}

function Header(s: StudyState, sc: ScreenColors) {
  const where = s.replay ? "FORT REVIEW" : `LEG ${Math.min(s.leg + 1, RUN_LEGS)}/${RUN_LEGS}`;
  const left = s.phase === "result" || s.phase === "over" ? `SCORE ${s.score}` : where;
  return (
    <g>
      <Line x={10} y={13} c={sc.fg} size={9}>{left}</Line>
      {/* The team: one block per ox still pulling. */}
      <g aria-label={`${s.oxen} of ${OXEN} oxen`}>
        {Array.from({ length: OXEN }, (_, i) => (
          <rect key={i} x={72 + i * 7} y={7} width={5} height={5} fill={i < s.oxen ? sc.good : sc.frame} opacity={i < s.oxen ? 0.9 : 0.35} />
        ))}
      </g>
      <Line x={W / 2 + 10} y={13} c={sc.dim} size={8} anchor="middle">{wrapText(regionName(s), 26, 1)[0]}</Line>
      <Line x={W - 10} y={13} c={sc.accent} size={9} anchor="end">{levelLine(s.profile)}</Line>
      <ProgressStrip frac={clamp(s.leg / RUN_LEGS, 0, 1)} marks={STRIP_MARKS} sc={sc} />
    </g>
  );
}

/** The header's middle: the region the question on screen is from. Batches
    can come from different regions, so this follows the question, not the
    newest batch's label. */
function regionName(s: StudyState) {
  const id = s.current?.territory;
  const region = id ? s.profile?.territories.find((t) => t.id === id) : undefined;
  return region?.name || s.label || modeName(s.mode);
}

function sourceLabel(q: StudyQuestion) {
  return q.source_lines ? `${q.source_path}:${q.source_lines[0]}-${q.source_lines[1]}` : q.source_path;
}

function ModeScreen(s: StudyState, sc: ScreenColors) {
  const p = s.profile;
  const sub = p
    ? `Level ${p.level} · ${pct(p.understanding)} of this codebase understood`
    : s.profileError
      ? "Couldn't read your progress — you can still ride."
      : "Answer questions about this codebase while your agent works.";
  const due = p?.due ?? 0;
  return (
    <g>
      <PxText x={W / 2} y={18} size={12} fill={sc.accent} shadow={sc.bg} anchor="middle">CHOOSE YOUR TRAIL</PxText>
      <Line x={W / 2} y={32} c={sc.dim} size={BODY} anchor="middle">{sub}</Line>
      {MODES.map((m, i) => {
        const blurb = m.key === "review" && due > 0 ? `${due} ${due === 1 ? "review is" : "reviews are"} due` : m.blurb;
        return (
          <g key={m.key}>
            <Line x={18} y={MODE_TOP + i * MODE_STEP} c={sc.accent} size={9}>{`${i + 1}  ${m.name}`}</Line>
            <Line x={108} y={MODE_TOP + i * MODE_STEP} c={m.key === "review" && due > 0 ? sc.good : sc.fg} size={BODY}>{blurb}</Line>
          </g>
        );
      })}
      <Line x={W / 2} y={H - 8} c={sc.dim} size={7} anchor="middle">
        {s.best > 0 ? `${RUN_LEGS} questions a run · best score ${s.best}` : `${RUN_LEGS} questions a run · the model writes them from your code`}
      </Line>
    </g>
  );
}

function ScoutingScreen(s: StudyState, sc: ScreenColors, p: ThemePalette) {
  const c = sceneColors(p);
  const dots = ".".repeat(1 + (Math.floor(s.clock * 2) % 3));
  return (
    <g>
      {Header(s, sc)}
      <svg x={80} y={32} width={128} height={45} viewBox="0 0 288 136" preserveAspectRatio="xMidYMid slice">
        <Vista c={c} horizon={18} travel={s.clock * 40} />
        <Pine x={8} y={31} c={c} scale={1.8} />
        <Wagon x={26} y={23} c={c} />
      </svg>
      <PxText x={W / 2} y={94} size={11} fill={sc.accent} shadow={sc.bg} anchor="middle">{`SCOUTING THE TERRITORY${dots}`}</PxText>
      <Line x={W / 2} y={108} c={sc.dim} size={7} anchor="middle">{`${modeName(s.mode)}: the study model is reading the code`}</Line>
      <Line x={W / 2} y={118} c={sc.dim} size={7} anchor="middle">and writing your questions. A few seconds.</Line>
    </g>
  );
}

const KIND_LABEL: Record<StudyQuestion["kind"], string> = {
  multiple_choice: "PICK ONE",
  true_false: "TRUE OR FALSE",
  free_text: "SHORT ANSWER",
  order: "PUT IN ORDER",
};

function QuestionScreen(s: StudyState, sc: ScreenColors) {
  const q = s.current;
  if (!q) return <g>{Header(s, sc)}</g>;
  const { prompt, rows, end } = layoutQuestion(q);
  const waiting = s.phase === "grading";
  const typed = !isChoice(q);
  const footer = waiting
    ? "grading…"
    : isChoice(q)
      ? `1-${q.options.length} answer   H hint (half points)`
      : "⏎ answer   ? hint (half points)";
  return (
    <g>
      {Header(s, sc)}
      <Line x={10} y={31} c={sc.dim} size={SMALL}>{`${KIND_LABEL[q.kind] ?? "QUESTION"} · DIFFICULTY ${clamp(q.difficulty, 1, 3)}/3${q.cached ? " · FROM YOUR NOTES" : ""}${s.hintUsed ? " · HINT USED" : ""}`}</Line>
      {prompt.map((line, i) => (
        <Line key={i} x={10} y={PROMPT_TOP + i * LINE_STEP} c={sc.fg} size={BODY}>{line}</Line>
      ))}
      {rows.map((row, i) => (
        <g key={i}>
          <Line x={12} y={row.y} c={sc.accent} size={BODY}>{`${i + 1}`}</Line>
          {row.lines.map((line, j) => (
            <Line key={j} x={24} y={row.y + j * LINE_STEP} c={waiting ? sc.dim : sc.good} size={BODY}>{line}</Line>
          ))}
        </g>
      ))}
      {typed && (
        <Line x={10} y={Math.min(end + 6, H - 18)} c={sc.accent} size={BODY}>
          {waiting
            ? "The study model is reading your answer…"
            : isOrder(q)
              ? "▶ Type the step numbers in order in the box below."
              : "▶ Type your answer in the box below."}
        </Line>
      )}
      <Footer sc={sc}>{footer}</Footer>
    </g>
  );
}

function HintScreen(s: StudyState, sc: ScreenColors) {
  const q = s.current;
  if (!q) return <g />;
  const cols = textCols(W - 44, SMALL);
  const lines = (q.source_excerpt || "(no excerpt — press O to read the file)").split("\n").slice(0, 8);
  return (
    <g>
      {Header(s, sc)}
      <CardFrame y={27} height={93} tone={sc.accent} />
      <CardTitle y={40} tone={sc.accent} sc={sc} size={9}>FROM THE SOURCE</CardTitle>
      <Line x={W / 2} y={50} c={sc.accent} size={SMALL} anchor="middle">{wrapText(sourceLabel(q), cols, 1)[0]}</Line>
      {lines.map((line, i) => (
        <Line key={i} x={22} y={61 + i * 7.5} c={sc.fg} size={SMALL}>{line.replace(/\t/g, "  ").slice(0, cols)}</Line>
      ))}
      <Footer sc={sc}>O opens the file   any other key returns to the question</Footer>
    </g>
  );
}

function ResultScreen(s: StudyState, sc: ScreenColors) {
  const g = s.grade;
  const q = s.current;
  if (!g || !q) return <g>{Header(s, sc)}</g>;
  const full = g.verdict === "full";
  const tone = s.flagged ? sc.dim : full ? sc.good : g.verdict === "partial" ? sc.accent : sc.bad;
  const title = s.flagged ? "STRUCK FROM THE RECORD" : full ? "RIGHT!" : g.verdict === "partial" ? "CLOSE" : "NOT QUITE";
  const cols = textCols(W - 44, 7);
  const small = textCols(W - 44, SMALL);
  const blocks: { text: string; c: string; size: number }[] = [];
  if (s.flagged) {
    blocks.push({ text: "This question won't be asked again, and your answer", c: sc.fg, size: 7 });
    blocks.push({ text: "to it no longer counts for or against you.", c: sc.fg, size: 7 });
  } else {
    if (s.leveledUp) blocks.push({ text: `▲ LEVEL UP — YOU ARE NOW LEVEL ${s.profile?.level ?? ""}`, c: sc.good, size: 7 });
    for (const line of wrapText(g.feedback, cols, 2)) blocks.push({ text: line, c: sc.fg, size: 7 });
    if (!full) for (const line of wrapText(`Answer: ${g.correct_answer}`, cols, 2)) blocks.push({ text: line, c: sc.accent, size: 7 });
    for (const line of wrapText(g.explanation, small, 3)) blocks.push({ text: line, c: sc.dim, size: SMALL });
  }
  const tally = s.flagged ? "" : `+${s.gained}${s.streak > 1 ? ` · STREAK ${s.streak}` : ""}${s.oxLost ? " · LOST AN OX" : ""}`;
  return (
    <g>
      {Header(s, sc)}
      <CardFrame y={27} height={93} tone={tone} />
      <CardTitle y={42} tone={tone} sc={sc}>{title}</CardTitle>
      {blocks.slice(0, 7).map((b, i) => (
        <Line key={i} x={W / 2} y={54 + i * 8} c={b.c} size={b.size} anchor="middle">{b.text}</Line>
      ))}
      <Line x={22} y={115} c={sc.accent} size={SMALL}>{wrapText(sourceLabel(q), Math.max(20, small - 22), 1)[0]}</Line>
      <Line x={W - 22} y={115} c={tone} size={7} anchor="end">{tally}</Line>
      <Footer sc={sc}>{s.flagged ? "O open the file   any other key rides on" : "O open the file   F this question is wrong   any other key rides on"}</Footer>
    </g>
  );
}

function MessageScreen(s: StudyState, sc: ScreenColors) {
  const tone = s.msgTone === "good" ? sc.good : s.msgTone === "bad" ? sc.bad : sc.accent;
  return (
    <g>
      {Header(s, sc)}
      <CardFrame y={38} height={72} tone={tone} />
      <CardTitle y={58} tone={tone} sc={sc}>{s.msgTitle}</CardTitle>
      {s.msgLines.map((line, i) => (
        <Line key={i} x={W / 2} y={74 + i * 11} c={sc.fg} size={8} anchor="middle">{line}</Line>
      ))}
      <Footer sc={sc}>press any key to continue</Footer>
    </g>
  );
}

function ErrorScreen(s: StudyState, sc: ScreenColors) {
  return (
    <g>
      <CardFrame y={20} height={96} tone={sc.bad} />
      <CardTitle y={40} tone={sc.bad} sc={sc}>TRAIL BLOCKED</CardTitle>
      {wrapText(s.errorText, textCols(W - 44, 7), 6).map((line, i) => (
        <Line key={i} x={W / 2} y={56 + i * 9} c={sc.fg} size={7} anchor="middle">{line}</Line>
      ))}
      <Footer sc={sc}>{s.errorBack === "mode" ? "press any key to pick another trail" : "any key tries again   S skips this question"}</Footer>
    </g>
  );
}

function OverScreen(s: StudyState, sc: ScreenColors) {
  const p = s.profile;
  const level = p?.level ?? s.startLevel;
  const now = p?.understanding ?? s.startUnderstanding;
  const delta = now - s.startUnderstanding;
  const asked = s.cutShort ? s.leg : RUN_LEGS;
  return (
    <g>
      <PxText x={W / 2} y={28} size={15} fill={sc.good} shadow={sc.bg} anchor="middle">{s.newBest ? "NEW BEST!" : s.cutShort ? "THE TRAIL RAN OUT" : "TRAIL'S END"}</PxText>
      <Line x={W / 2} y={44} c={sc.fg} size={9} anchor="middle">{`${s.correct} of ${asked} answered in full · best streak ${s.bestStreak}`}</Line>
      <Line x={W / 2} y={57} c={delta >= 0 ? sc.good : sc.bad} size={9} anchor="middle">
        {`Understanding ${s.startUnderstanding.toFixed(1)}% → ${now.toFixed(1)}%`}
      </Line>
      <Line x={W / 2} y={69} c={level > s.startLevel ? sc.good : sc.dim} size={8} anchor="middle">
        {level > s.startLevel ? `Level ${s.startLevel} → ${level}` : `Level ${level}`}
      </Line>
      <Line x={W / 2} y={82} c={s.oxen > 0 ? sc.good : sc.dim} size={7} anchor="middle">
        {s.oxen > 0 ? `${s.oxen} of ${OXEN} oxen made it: +${s.oxen * OX_BONUS}` : "No oxen made it — you walked the last miles."}
      </Line>
      <PxText x={W / 2} y={97} size={13} fill={sc.accent} shadow={sc.bg} anchor="middle">{`SCORE ${s.score}   BEST ${s.best}`}</PxText>
      {s.rank > 0 && <Line x={W / 2} y={108} c={sc.good} size={8} anchor="middle">{rankWord(s.rank)}</Line>}
      {s.cutShort && s.batchError ? (
        <Line x={W / 2} y={118} c={sc.bad} size={SMALL} anchor="middle">{wrapText(s.batchError, textCols(TEXT_W, SMALL), 1)[0]}</Line>
      ) : (
        s.tokens > 0 && <Line x={W / 2} y={118} c={sc.dim} size={SMALL} anchor="middle">{`${s.tokens.toLocaleString()} tokens spent writing and grading`}</Line>
      )}
      <Footer sc={sc}>press any key to ride again</Footer>
    </g>
  );
}

function StudyRender(s: StudyState, p: ThemePalette) {
  const sc = screenColors(p);
  let body: React.JSX.Element;
  let label: string;
  switch (s.phase) {
    case "mode":
      body = ModeScreen(s, sc);
      label = "Trail of Understanding: choose a trail with 1 to 4";
      break;
    case "scouting":
      body = ScoutingScreen(s, sc, p);
      label = "Trail of Understanding: writing your questions";
      break;
    case "hint":
      body = HintScreen(s, sc);
      label = `Trail of Understanding: a hint from ${s.current?.source_path ?? "the source"}`;
      break;
    case "result":
      body = ResultScreen(s, sc);
      label = `Trail of Understanding: ${s.grade?.verdict === "full" ? "correct" : s.grade?.verdict === "partial" ? "partly correct" : "incorrect"}. ${s.grade?.feedback ?? ""}`;
      break;
    case "message":
      body = MessageScreen(s, sc);
      label = `Trail of Understanding: ${s.msgTitle}`;
      break;
    case "error":
      body = ErrorScreen(s, sc);
      label = `Trail of Understanding: ${s.errorText}`;
      break;
    case "over":
      body = OverScreen(s, sc);
      label = `Trail of Understanding: run over, score ${s.score}`;
      break;
    default:
      body = QuestionScreen(s, sc);
      label = `Trail of Understanding: ${s.current?.prompt ?? "a question"}`;
  }
  return (
    <GameFrame label={label}>
      <Px x={0} y={0} w={COLS} h={ROWS} fill={sc.bg} />
      <rect x={0} y={0} width={W} height={2} fill={sc.accent} opacity={0.5} />
      {body}
    </GameFrame>
  );
}

/** Attract screen: the trail's title card, with your standing on this
    project — the level, and one bar per region showing what you know. */
function StudyAttract(p: ThemePalette, s: StudyState) {
  const c = sceneColors(p);
  const profile = s?.profile ?? null;
  const regions = profile?.territories ?? [];
  const barsX = 8;
  const barsW = 196;
  const slot = regions.length ? barsW / regions.length : 0;
  return (
    <GameFrame label="Trail of Understanding title screen: learn the codebase your agent is writing">
      <Vista c={c} horizon={21} />
      {poly([[30, 21], [33, 21], [56, ROWS], [20, ROWS]], c.trail, 0.3)}
      <Pine x={6} y={30} c={c} scale={2.1} />
      <Pine x={16} y={24} c={c} scale={1.1} />
      <Wagon x={27} y={24} c={c} />
      <TitlePlaque c={c} eyebrow="04 / PRAIRIE ARCADE · KNOW THE CODE YOUR AGENT WRITES" title="TRAIL OF UNDERSTANDING" subtitle={`${RUN_LEGS} QUESTIONS · YOUR CODEBASE · WRITTEN FROM THE SOURCE`} />
      <rect x={211} y={76} width={72} height={42} fill={c.sky} opacity={0.8} />
      {profile ? (
        <g>
          <Line x={W - 10} y={86} c={c.accent} size={6} anchor="end">YOUR STANDING</Line>
          <Line x={W - 10} y={97} c={c.snow} size={9} anchor="end">{`LEVEL ${profile.level}`}</Line>
          <Line x={W - 10} y={106} c={c.snow} size={6} anchor="end">{`${pct(profile.understanding)} UNDERSTOOD`}</Line>
          <Line x={W - 10} y={114} c={c.snow} size={6} anchor="end">{`${profile.answered} ANSWERED`}</Line>
        </g>
      ) : (
        <g>
          <Line x={W - 10} y={90} c={c.accent} size={6} anchor="end">YOUR STANDING</Line>
          <Line x={W - 10} y={102} c={c.snow} size={6} anchor="end">PLAY TO MAP WHAT</Line>
          <Line x={W - 10} y={110} c={c.snow} size={6} anchor="end">YOU UNDERSTAND</Line>
        </g>
      )}
      {regions.length > 0 && (
        <g aria-hidden="true">
          <rect x={barsX - 2} y={H - 28} width={barsW + 4} height={24} fill={c.sky} opacity={0.8} />
          <Line x={barsX} y={H - 22} c={c.accent} size={5}>{`MASTERY ACROSS ${regions.length} REGIONS`}</Line>
          {regions.map((t, i) => (
            <g key={t.id}>
              <title>{`${t.id}: ${pct(t.mastery * 100)}`}</title>
              <rect x={barsX + i * slot} y={H - 7} width={Math.max(1, slot - 1)} height={1} fill={c.line} opacity={0.6} />
              <rect
                x={barsX + i * slot}
                y={H - 7 - Math.round(t.mastery * 12)}
                width={Math.max(1, slot - 1)}
                height={Math.round(t.mastery * 12)}
                fill={t.faded >= FADING ? c.danger : c.grass}
              />
            </g>
          ))}
        </g>
      )}
    </GameFrame>
  );
}

export const StudyGame: HeroGameDefinition<StudyState> = {
  title: "Trail of Understanding",
  tab: "Study",
  initialState: () => fresh(),
  onStart: (s) => fresh(Math.max(s.best, loadBest("study")), s.profile, s.requests),
  handleKey: studyKey,
  handlePointer: studyPointer,
  deliver: studyDeliver,
  textEntry: studyEntry,
  handleText: studyText,
  update: (s, dt) => ({ ...s, clock: s.clock + dt }),
  render: StudyRender,
  renderAttract: StudyAttract,
  keys: (key) => key.length === 1 || key === "Enter",
  noDaily: true,
};

export function loadStudyTable() {
  return loadTable(TABLE_KEY, SEED_TABLE);
}
