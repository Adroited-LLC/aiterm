/**
 * Driving a TUI list from a dialog.
 *
 * Both screens aiterm dresses up — the model picker and the permission prompt —
 * are the same widget underneath: claude's `Select`, whose bindings are
 *
 *   up/down/j/k/ctrl+n/ctrl+p → move · enter → accept · escape → cancel
 *
 * Notably absent: digits and letters. A number key does nothing, which is worth
 * knowing because the rows are numbered and pressing `3` looks like it ought to
 * choose the third one. It does not, and the Enter that follows accepts
 * whatever is still highlighted.
 *
 * Which is the whole reason this is shared rather than written twice. Enter
 * commits the *highlighted* row, so being sure the highlight is where we think
 * it is before pressing it is the entire safety property. One implementation,
 * closed-loop, used by both.
 */

const KEY_DOWN = "\x1b[B";
const KEY_UP = "\x1b[A";
/** Long enough for claude to repaint between presses on a loaded machine. */
export const SETTLE_MS = 90;
/** More than any real list needs; stops a mis-detection spinning for ever. */
const MAX_STEPS = 24;

export const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * Move the highlight onto `target`, confirming from the screen after every
 * press. Returns the freshly-read state once it is there, or throws — never
 * returns having failed quietly.
 */
export async function moveHighlight<T extends { highlighted: number }>(
  read: () => T | null,
  write: (data: string) => void,
  target: number,
): Promise<T> {
  for (let step = 0; step < MAX_STEPS; step++) {
    const now = read();
    if (!now) throw new Error("the screen changed before the choice landed");
    if (now.highlighted === target) return now;
    write(now.highlighted < target ? KEY_DOWN : KEY_UP);
    await wait(SETTLE_MS);
  }
  throw new Error("could not move the selection to that row");
}

/**
 * Put the highlight on `target` and accept it.
 *
 * The re-read between arriving and pressing Enter is not redundant: it is the
 * last moment before a keystroke that commits, and the screen may have moved
 * since the loop's final look.
 */
export async function selectRow<T extends { highlighted: number }>(
  read: () => T | null,
  write: (data: string) => void,
  target: number,
): Promise<void> {
  await moveHighlight(read, write, target);
  const confirmed = read();
  if (!confirmed || confirmed.highlighted !== target) {
    throw new Error("the selection moved unexpectedly — nothing was sent");
  }
  write("\r");
}
