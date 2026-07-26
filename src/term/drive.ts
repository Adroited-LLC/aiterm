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

/** Shift+Tab — CSI Z, the standard back-tab. `chat:cycleMode` is bound to it. */
const KEY_BACKTAB = "\x1b[Z";

/**
 * Cycle a session's permission mode until it reads `target`.
 *
 * The cycle is a loop with no way to step backwards, so this presses forward
 * and re-reads, at most one full lap plus a little. Bounded rather than
 * hopeful: if the mode never arrives — most likely because bypass is not
 * enabled for this session — it says so instead of pressing for ever.
 */
export async function cycleModeTo(
  read: () => string | null,
  write: (data: string) => void,
  target: string,
  laps = 6,
): Promise<void> {
  for (let step = 0; step < laps; step++) {
    const now = read();
    if (now === target) return;
    if (now === null) throw new Error("could not read the current mode");
    write(KEY_BACKTAB);
    await wait(SETTLE_MS * 2);
  }
  if (read() !== target) {
    throw new Error(
      `could not reach "${target}" — it may not be enabled for this session`,
    );
  }
}

/**
 * Collect a scrolling TUI list that only renders a window at a time.
 *
 * The rewind picker can hold far more points than fit on screen, and we read
 * the viewport — so what is drawn is all we would otherwise know about.
 * Walking the highlight scrolls the window, and in a picker that commits
 * nothing until Enter, walking is free.
 *
 * Goes up to the top, gathering as it climbs, and stops when two consecutive
 * reads add nothing. Returns the full list in order, with the highlight parked
 * at the top so a later selection can walk down deterministically.
 */
export async function harvestUpwards<T>(
  read: () => { items: T[]; highlighted: number; atTop?: boolean } | null,
  write: (data: string) => void,
  key: (item: T) => string,
  maxSteps = 200,
  onProgress?: (count: number) => void,
): Promise<T[]> {
  const seen = new Map<string, T>();
  const order: string[] = [];
  const absorb = (items: T[]) => {
    let added = 0;
    // Prepend: climbing reveals earlier entries, so new ones belong in front.
    for (let i = items.length - 1; i >= 0; i--) {
      const k = key(items[i]);
      if (seen.has(k)) continue;
      seen.set(k, items[i]);
      order.unshift(k);
      added++;
    }
    return added;
  };

  const first = read();
  if (!first) throw new Error("the list went away");
  absorb(first.items);
  onProgress?.(order.length);

  let quiet = 0;
  for (let step = 0; step < maxSteps; step++) {
    const before = read();
    if (!before) break;
    // Prefer the list's own "nothing above" signal; fall back to "two reads
    // in a row told us nothing new" where a list does not offer one.
    if (before.atTop === true) break;
    if (before.atTop === undefined && quiet >= 2) break;
    write(KEY_UP);
    await wait(SETTLE_MS);
    const after = read();
    if (!after) break;
    quiet = absorb(after.items) === 0 ? quiet + 1 : 0;
    onProgress?.(order.length);
  }
  return order.map((k) => seen.get(k)!);
}

/**
 * Move the highlight onto the row whose identity matches, rather than onto an
 * index. After harvesting, our list and the rendered window no longer share a
 * coordinate system — the visible list is a slice — so an index would point at
 * the wrong row. Matching on what the row *says* cannot drift.
 */
export async function selectByIdentity<T>(
  read: () => { items: T[]; highlighted: number } | null,
  write: (data: string) => void,
  key: (item: T) => string,
  target: string,
  maxSteps = 200,
): Promise<void> {
  for (let step = 0; step < maxSteps; step++) {
    const now = read();
    if (!now) throw new Error("the list went away before the choice landed");
    if (key(now.items[now.highlighted]) === target) return;
    write(KEY_DOWN);
    await wait(SETTLE_MS);
  }
  throw new Error("could not find that point in the list");
}
