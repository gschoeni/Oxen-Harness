# Arcade delight — 2026-09-18

## Intent and scope

Improve all three existing cabinets without replacing their pixel art or adding
an engine/dependency. Preserve theme colors, local bests, sound preferences,
keyboard/touch play, and the quiet default attract screen. Existing unrelated
checkout changes are outside this feature.

## Research → design

- Steve Swink, [Principles of Virtual Sensation](https://www.gamedeveloper.com/design/principles-of-virtual-sensation):
  readable response and moment-to-moment control come first. Give Hunt a visible
  aim guide and make taps aim where the player points.
- Maddy Thorson, [Celeste & Forgiveness](https://www.mattmakesgames.com/articles/celeste_and_forgiveness/index.html):
  small timing allowances help challenging controls feel fair. Buffer Dodge hops
  and protect the player briefly after a shield breaks.
- Martin Jonasson / Petri Purho, [Juice It or Lose It](https://www.gdcvault.com/play/1016789/Juice-It-or-Lose):
  reinforce actions with coordinated sound, motion and particles. Dodge's earned
  Stampede and Hunt's focused shot get distinct audiovisual responses.
- Pichlmair / Johansen, [Designing Game Feel: A Survey](https://arxiv.org/abs/2011.09201):
  amplification should communicate the importance of events. Keep effects below
  readable HUDs, respect reduced motion, and use clear progress toward mastery.

These sources inform design choices; they do not establish that these particular
games are fun. Automated regression checks and browser playtests verify behavior
and presentation; longer human playtesting remains useful for balance.

## Implementation

- Dodge: coins and close calls charge a five-second Stampede (coin magnet,
  obstacle smashing, doubled points); buffered hop, shield recovery, rich vista.
- Hunt: stand still to focus a powerful shot, point-to-aim taps, consecutive hit
  bonuses, field texture, shadows, aim feedback and readable trip summaries.
- Trail: campsite recovery/foraging/scouting with explicit costs, next-leg
  forecast, visible river risks, scenic dashboard and correct landmark order.
- Cabinet: controls outside the art, explicit start/pause/menu controls, keyboard
  focus and pause when returning to the composer, large playable dock.

## Verification and review

Pending implementation, regression suite, browser inspection, workspace checks,
build-cache cleanup, feature commit and dedicated review/refactor pass.

### Feature verification

- Wrote regressions first: 10 new mechanics checks failed before implementation.
- TypeScript and the production Vite build pass (existing bundle-size warning).
- All 81 arcade/wrapper tests pass. Full frontend suite: 599 pass, 2 unrelated
  sizing-ratchet failures in `styles/tokens.test.ts` (other modified stylesheets
  and stale baseline entries). New arcade CSS uses the project's size tokens.
- Chrome: each cabinet starts, pauses/resumes, returns to menu, and pauses on
  composer focus; Trail camp actions work; 390px viewport has no overflow.
- WebKit: Hunt controls/pause pass with reduced motion enabled.
- Scripted simulations finished all five Hunt trips for seeds 1, 42 and 99;
  a Dodge run reached 60 seconds and six Stampedes at seed 42. This is a smoke
  check, not a player study or a claim of calibrated difficulty.
- Workspace fmt passes. Initial workspace Clippy failed in existing protocol
  wire tests (media/thread/retry schema mismatch); `cargo test --workspace`
  reached CLI tests and failed 18 shared-state composer/completion cases.
  Bridge Clippy found an existing redundant closure in `commands/files.rs:834`.
  Nextest is being checked separately to isolate the CLI tests.
- Requested cache GC was run; its warm commands succeeded, but pruning raced
  a disappearing object file (`FileNotFoundError`). Retry after final checks.
- Temporary browser fixtures/screenshots live outside the feature commit.
