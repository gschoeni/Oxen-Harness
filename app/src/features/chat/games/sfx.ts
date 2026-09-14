// Tiny Web Audio synth for the hero games. Everything is a short oscillator
// envelope — square and triangle blips, a noise burst for crashes — so there are
// no assets to load and the whole thing is a few hundred bytes of code. Games
// never call this directly: they queue named cues on their state (`pushSfx` in
// gameKit) and the HeroGame wrapper plays whatever is new after each update.
//
// Sound is opt-in per player (localStorage `oxen-hero-sound`, default on in the
// hero) and always starts muted in the floating dock so a streaming turn stays
// quiet unless you flip it on for the session.

export type SfxName =
  | "coin"
  | "chain"
  | "graze"
  | "hop"
  | "land"
  | "crash"
  | "shield"
  | "shieldHit"
  | "milestone"
  | "best"
  | "start"
  | "shot"
  | "hit"
  | "kill"
  | "bigKill"
  | "empty"
  | "full"
  | "menu"
  | "good"
  | "bad"
  | "tick"
  | "growl";

const PREF_KEY = "oxen-hero-sound";

let ctx: AudioContext | null = null;
let master: GainNode | null = null;
let noiseBuffer: AudioBuffer | null = null;

function audio(): AudioContext | null {
  if (ctx) return ctx;
  try {
    const Ctor = (window as any).AudioContext || (window as any).webkitAudioContext;
    if (!Ctor) return null;
    ctx = new Ctor() as AudioContext;
    master = ctx.createGain();
    master.gain.value = 0.16;
    master.connect(ctx.destination);
    return ctx;
  } catch {
    return null;
  }
}

/** The player's saved preference (hero cabinet default). */
export function sfxPreference(): boolean {
  try {
    const raw = window.localStorage?.getItem(PREF_KEY);
    return raw == null ? true : raw === "1";
  } catch {
    return true;
  }
}

export function setSfxPreference(on: boolean) {
  try {
    window.localStorage?.setItem(PREF_KEY, on ? "1" : "0");
  } catch {
    // preference only
  }
}

/** Browsers gate audio behind a user gesture; call this from a key/click handler. */
export function unlockSfx() {
  const a = audio();
  if (a && a.state === "suspended") void a.resume().catch(() => undefined);
}

function noise(a: AudioContext) {
  if (noiseBuffer) return noiseBuffer;
  const len = Math.floor(a.sampleRate * 0.25);
  noiseBuffer = a.createBuffer(1, len, a.sampleRate);
  const data = noiseBuffer.getChannelData(0);
  for (let i = 0; i < len; i++) data[i] = Math.random() * 2 - 1;
  return noiseBuffer;
}

interface Tone {
  type: OscillatorType;
  from: number; // Hz
  to?: number; // Hz, glides if set
  dur: number; // seconds
  vol?: number;
  delay?: number;
}

function tone(a: AudioContext, t: Tone) {
  if (!master) return;
  const t0 = a.currentTime + (t.delay ?? 0);
  const osc = a.createOscillator();
  const gain = a.createGain();
  osc.type = t.type;
  osc.frequency.setValueAtTime(t.from, t0);
  if (t.to) osc.frequency.exponentialRampToValueAtTime(Math.max(1, t.to), t0 + t.dur);
  gain.gain.setValueAtTime(t.vol ?? 1, t0);
  gain.gain.exponentialRampToValueAtTime(0.001, t0 + t.dur);
  osc.connect(gain);
  gain.connect(master);
  osc.start(t0);
  osc.stop(t0 + t.dur + 0.02);
}

function burstNoise(a: AudioContext, dur: number, vol = 1, delay = 0) {
  if (!master) return;
  const t0 = a.currentTime + delay;
  const src = a.createBufferSource();
  src.buffer = noise(a);
  const gain = a.createGain();
  gain.gain.setValueAtTime(vol, t0);
  gain.gain.exponentialRampToValueAtTime(0.001, t0 + dur);
  const filter = a.createBiquadFilter();
  filter.type = "lowpass";
  filter.frequency.setValueAtTime(1800, t0);
  filter.frequency.exponentialRampToValueAtTime(200, t0 + dur);
  src.connect(filter);
  filter.connect(gain);
  gain.connect(master);
  src.start(t0);
  src.stop(t0 + dur + 0.02);
}

// The cue sheet. Frequencies are picked so cues that fire together (coin +
// chain, kill + full) still read as two events.
const CUES: Record<SfxName, (a: AudioContext) => void> = {
  coin: (a) => tone(a, { type: "square", from: 1046, to: 1568, dur: 0.09, vol: 0.5 }),
  chain: (a) => {
    tone(a, { type: "square", from: 784, dur: 0.07, vol: 0.5 });
    tone(a, { type: "square", from: 1046, dur: 0.07, vol: 0.5, delay: 0.07 });
    tone(a, { type: "square", from: 1568, dur: 0.12, vol: 0.5, delay: 0.14 });
  },
  graze: (a) => tone(a, { type: "triangle", from: 520, to: 880, dur: 0.12, vol: 0.7 }),
  hop: (a) => tone(a, { type: "square", from: 300, to: 620, dur: 0.14, vol: 0.4 }),
  land: (a) => burstNoise(a, 0.08, 0.35),
  crash: (a) => {
    burstNoise(a, 0.35, 1);
    tone(a, { type: "sawtooth", from: 160, to: 40, dur: 0.4, vol: 0.8 });
  },
  shield: (a) => {
    tone(a, { type: "triangle", from: 660, dur: 0.1, vol: 0.6 });
    tone(a, { type: "triangle", from: 990, dur: 0.18, vol: 0.6, delay: 0.09 });
  },
  shieldHit: (a) => {
    burstNoise(a, 0.15, 0.6);
    tone(a, { type: "square", from: 880, to: 220, dur: 0.25, vol: 0.6 });
  },
  milestone: (a) => {
    tone(a, { type: "square", from: 523, dur: 0.1, vol: 0.5 });
    tone(a, { type: "square", from: 659, dur: 0.1, vol: 0.5, delay: 0.1 });
    tone(a, { type: "square", from: 784, dur: 0.22, vol: 0.5, delay: 0.2 });
  },
  best: (a) => {
    tone(a, { type: "square", from: 659, dur: 0.09, vol: 0.5 });
    tone(a, { type: "square", from: 880, dur: 0.09, vol: 0.5, delay: 0.09 });
    tone(a, { type: "square", from: 1175, dur: 0.09, vol: 0.5, delay: 0.18 });
    tone(a, { type: "square", from: 1568, dur: 0.3, vol: 0.5, delay: 0.27 });
  },
  start: (a) => {
    tone(a, { type: "square", from: 440, dur: 0.08, vol: 0.4 });
    tone(a, { type: "square", from: 880, dur: 0.16, vol: 0.4, delay: 0.1 });
  },
  shot: (a) => {
    burstNoise(a, 0.12, 0.9);
    tone(a, { type: "square", from: 220, to: 60, dur: 0.1, vol: 0.5 });
  },
  hit: (a) => tone(a, { type: "triangle", from: 440, to: 330, dur: 0.1, vol: 0.6 }),
  kill: (a) => {
    tone(a, { type: "square", from: 587, dur: 0.07, vol: 0.5 });
    tone(a, { type: "square", from: 880, dur: 0.14, vol: 0.5, delay: 0.07 });
  },
  bigKill: (a) => {
    burstNoise(a, 0.3, 0.8);
    tone(a, { type: "sawtooth", from: 110, to: 55, dur: 0.35, vol: 0.7 });
    tone(a, { type: "square", from: 880, dur: 0.18, vol: 0.4, delay: 0.25 });
  },
  empty: (a) => tone(a, { type: "square", from: 200, dur: 0.05, vol: 0.3 }),
  full: (a) => {
    tone(a, { type: "square", from: 659, dur: 0.1, vol: 0.5 });
    tone(a, { type: "square", from: 784, dur: 0.1, vol: 0.5, delay: 0.1 });
    tone(a, { type: "square", from: 1046, dur: 0.25, vol: 0.5, delay: 0.2 });
  },
  menu: (a) => tone(a, { type: "square", from: 660, dur: 0.04, vol: 0.3 }),
  good: (a) => {
    tone(a, { type: "triangle", from: 523, dur: 0.1, vol: 0.5 });
    tone(a, { type: "triangle", from: 784, dur: 0.2, vol: 0.5, delay: 0.1 });
  },
  bad: (a) => {
    tone(a, { type: "triangle", from: 330, dur: 0.15, vol: 0.5 });
    tone(a, { type: "triangle", from: 220, dur: 0.3, vol: 0.5, delay: 0.15 });
  },
  tick: (a) => tone(a, { type: "square", from: 1200, dur: 0.03, vol: 0.25 }),
  growl: (a) => {
    tone(a, { type: "sawtooth", from: 90, to: 70, dur: 0.3, vol: 0.6 });
    burstNoise(a, 0.2, 0.3, 0.05);
  },
};

export function playSfx(name: SfxName) {
  const a = audio();
  if (!a || a.state !== "running") return;
  try {
    CUES[name]?.(a);
  } catch {
    // Never let audio take down a frame.
  }
}
