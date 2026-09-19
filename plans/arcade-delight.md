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

Implemented and committed as `cb2a611`. The dedicated review and polish are
complete. Arcade verification is green; the broader checkout has the unrelated
failures listed in the final verification record below.

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
  An initial isolated Nextest run passed 1,331 tests (5 skipped); the final
  rerun encountered the store/host mismatch recorded below.
- Requested cache GC was run; its warm commands succeeded, but pruning raced
  a disappearing object file (`FileNotFoundError`). Retry after final checks.
- Temporary browser fixtures/screenshots live outside the feature commit.

### Dedicated review of `cb2a611`

Concrete findings (modularity, maintainability, readability, idiomatic frontend
code, and pragmatism):

1. **Keep scenery cheap:** static field texture and quantized vista geometry
   currently rebuild hundreds of SVG elements on every game tick. Memoize these
   two bounded renderers using palette values and their actual animation inputs.
2. **Keep taps intentional:** the Trail's clamped menu hit mapping treats taps
   on scenery as Travel. Restrict menu input to the drawn rows; remove the
   unreachable duplicate Hunt pointer branch.
3. **Preserve useful status:** restore the last-leg/ailment readout below the
   Trail scene; show health and full names in the travelers' SVG descriptions.
4. **Contain input:** two mounted cabinets should not resume each other. Pause
   when focus moves into another cabinet, and route its keyboard input only to
   that cabinet. Reuse one pause/resume helper for keyboard and toolbar.
5. **Finish Hunt cleanly:** stop accepting new shots during the full-bag ending,
   keep focus consistent during warmup, and extract the aim guide so its bounds
   and terrain occlusion are readable independently of the field renderer.
6. **Reject repeated camp actions before affordability checks:** scouting an
   already-scouted leg must be a no-op even when food has since run out.

7. **Give river safety a real tradeoff:** floating was strictly better than
   fording at the same cost. Require one caulking kit and one day, show both
   costs, and preserve an explicit no-supplies error that returns to the river.

Keep the existing small state machines and SVG renderer; an engine, generalized
entity/component layer, or a separate progression service would add complexity
without helping this feature. The new art is shared; game-specific scoring and
input remain beside their tests. Apply the seven concrete improvements above and
re-run checks in a separate polish commit.


### Final verification after polish

- All **88 arcade tests pass** (21 Dodge, 27 Hunt, 30 Trail, 10 wrapper).
- TypeScript and the production build pass. Full frontend: **606 pass / 2 fail**;
  both failures are the existing CSS sizing-ratchet/baseline tests, with no
  arcade stylesheet violations. Existing bundle-size warning remains.
- Chrome and WebKit browser checks were repeated successfully after the review:
  controls, pause/resume, composer focus, menu return, Trail camp, narrow layout,
  and reduced motion. No browser runtime errors were observed.
- `cargo fmt --all -- --check` passes.
- Final workspace Clippy remains blocked by 20 protocol wire-test schema errors.
- Final Nextest cannot build `harness-host`: `ThreadRow`, `FINISHED_STATE`, and
  `thread_rows` are missing from the current store API. The earlier full
  1,331-test pass is not a green result for this final checkout.
- Bridge Clippy remains blocked by the unrelated redundant closure in
  `app/src-tauri/src/commands/files.rs:834`.
- Retried `scripts/gc-target.py`; it safely refused to prune when its workspace
  test build encountered the same store/host mismatch. No workaround or manual
  cache deletion was used.
- The review also preserves keyboard control after toggling sound, allows Tab
  out of a paused cabinet, and prevents one global start combo from launching
  two mounted cabinets. New regressions cover the latter two-cabinet behavior
  and sound-focus behavior.
- Existing staged renames and unrelated working changes remain outside both
  arcade commits. Temporary browser fixture files were removed after checking.
