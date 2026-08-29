export type TabId = string;

interface ExitEvent {
  tabId: TabId;
  code: number | null;
  signal: string | null;
}

interface ExitDescriptor {
  id: TabId;
  state?: "running" | "exited";
  exit?: { code: number | null; signal: string | null };
}

/** Deduplicate the live exit event and the authoritative attach-error catch-up.
 *
 * The listener is installed before desktop attach, but a process can already
 * have exited before the component mounts. A rejected attach therefore reads
 * the registry and feeds that descriptor through the same one-shot delivery
 * gate. Disposing the gate first makes a queued old-tab callback harmless when
 * React has already mounted a replacement.
 */
export function createTabExitCatchUp(
  tabId: TabId,
  onExit: (tabId: TabId, code: number | null, signal: string | null) => void,
) {
  let disposed = false;
  let delivered = false;

  const deliver = (code: number | null, signal: string | null) => {
    if (disposed || delivered) return;
    delivered = true;
    onExit(tabId, code, signal);
  };

  return {
    event(exit: ExitEvent) {
      if (exit.tabId === tabId) deliver(exit.code, exit.signal);
    },
    reconcile(descriptors: ExitDescriptor[]) {
      const descriptor = descriptors.find((candidate) => candidate.id === tabId);
      if (descriptor?.state === "exited") {
        deliver(descriptor.exit?.code ?? null, descriptor.exit?.signal ?? null);
      }
    },
    dispose() {
      disposed = true;
    },
  };
}

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
