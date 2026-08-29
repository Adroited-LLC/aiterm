export type TabId = string;

/** Replace the renderer's projection with Rust's authoritative tab list.
 *
 * Renderer-only fields on a still-live tab survive, while every field Rust
 * owns is replaced by the latest descriptor. Tabs omitted by Rust are gone.
 */
export function reconcileTabs<
  Current extends { id: TabId },
  Authoritative extends { id: TabId },
>(current: Current[], authoritative: Authoritative[]): Array<Current & Authoritative> {
  const currentById = new Map(current.map((tab) => [tab.id, tab]));
  return authoritative.map((descriptor) => ({
    ...(currentById.get(descriptor.id) as Current | undefined),
    ...descriptor,
  } as Current & Authoritative));
}
