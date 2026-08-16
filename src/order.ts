/** Display order that survives a refresh.
 *
 * The sidebar's rows arrive sorted by `last_active`, which moves every time an
 * agent writes a line to its transcript. With several sessions running, that
 * meant rows overtaking each other every few seconds while you were trying to
 * click one — the list rearranged itself under the cursor.
 *
 * Recency still decides where a row *arrives*: anything not seen before goes in
 * at the top, in the order it came. After that the row stays where it is until
 * it disappears. So a new session still shows up first, and an old one no
 * longer jumps the queue just because its agent said something.
 *
 * The same shape as the manual drag order in `SessionsPanel`, deliberately —
 * remembered order first, unseen items on top — so the two compose instead of
 * fighting.
 */
export function stableOrder<T>(items: T[], id: (t: T) => string, remembered?: string[]): T[] {
  if (!remembered || remembered.length === 0) return items;
  const rank = new Map(remembered.map((k, i) => [k, i]));
  const fresh = items.filter((t) => !rank.has(id(t)));
  const known = items
    .filter((t) => rank.has(id(t)))
    .sort((a, b) => rank.get(id(a))! - rank.get(id(b))!);
  return [...fresh, ...known];
}
