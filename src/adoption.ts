/** How long to keep looking for the session an agent named itself.
 *
 * An engine with no `--session-id` (Codex) cannot be told what to call itself,
 * so aiterm opens the tab against a placeholder and watches the store for the
 * session that appears in the directory it launched in. Until that lands, the
 * conversation owns two sidebar rows: the placeholder, and — once the agent
 * finally writes — the real scanned row.
 *
 * This used to give up after a flat minute, on the assumption that Codex wrote
 * its transcript at launch. It does not. Measured against codex-cli 0.147.0 on
 * 2026-08-16: a session that started at 13:26:08 had no rollout file until
 * 13:27:47 — 98.9s, half a minute past the old deadline, so adoption quit
 * before the file it was waiting for existed and the duplicate row became
 * permanent. Another session the same week took 24.5s, which is why this
 * misbehaved intermittently rather than always.
 *
 * The window is generous now because the file appears when the agent has
 * something to say, and someone can open a tab and leave it sitting. The cost
 * of waiting is one directory scan, so it backs off rather than stopping: every
 * couple of seconds while a transcript is plausibly imminent, then occasionally
 * for as long as someone might reasonably still be composing a first prompt.
 * Closing the tab ends it either way — that, not the clock, is the real bound.
 *
 * Returns the milliseconds to wait before looking again, or `null` to stop.
 */
export function nextAdoptionDelay(elapsedMs: number): number | null {
  if (elapsedMs < 0) return FAST_EVERY;
  if (elapsedMs < FAST_FOR) return FAST_EVERY;
  if (elapsedMs < GIVE_UP_AFTER) return SLOW_EVERY;
  return null;
}

/** Poll quickly for the first stretch: most sessions land inside it. */
export const FAST_FOR = 2 * 60_000;
export const FAST_EVERY = 2_000;
/** Then quietly, for as long as a first prompt is still plausible. */
export const GIVE_UP_AFTER = 30 * 60_000;
export const SLOW_EVERY = 15_000;
