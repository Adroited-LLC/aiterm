/** Which sidebar rows are selected, and what happens to that when the terminal
 *  under them changes identity.
 *
 *  A tab does not keep one session id for life. `/clear`, `/fork`, and picking
 *  another agent from claude's own agents screen all leave the terminal holding
 *  a *new* conversation, and aiterm re-keys the tab to it. The row highlight
 *  driven by `activeSlot` follows that automatically. The click-driven
 *  selection did not: it was written only by clicking a row, so it went on
 *  pointing at the conversation you left, wearing the strongest highlight of
 *  the three while the live row wore the faintest. */

/** Move a selection from one slot to another when the active tab re-keys.
 *
 *  Only rewrites a selection that actually contained `from` — a selection built
 *  elsewhere (ctrl-clicking rows for a drag) is somebody else's and is left
 *  exactly as it is. Deliberately a *move* and not an add: `selected` also
 *  drives multi-row drag, so quietly growing it would enlarge the next
 *  operation the user performs on it. */
export function followRekey(
  selected: ReadonlySet<string>,
  from: string | null,
  to: string | null,
): Set<string> {
  const unchanged = new Set(selected);
  if (!from || !to || from === to) return unchanged;
  if (!selected.has(from)) return unchanged;
  const next = new Set(selected);
  next.delete(from);
  next.add(to);
  return next;
}
