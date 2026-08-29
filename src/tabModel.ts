export type TabId = string;

export interface TabRegistryProjection<T extends { id: TabId }> {
  revision: number | null;
  tabs: T[];
}

type RegistryTab<T extends { id: TabId }> = Partial<T> & { id: TabId };

export type TabRegistryEvent<T extends { id: TabId }> =
  | { change: "snapshot"; revision: number; tabs: RegistryTab<T>[] }
  | { change: "opened" | "changed"; revision: number; tabId: TabId; tab: RegistryTab<T> }
  | { change: "removed"; revision: number; tabId: TabId; requested: boolean };

export interface TabRegistryApplyResult<T extends { id: TabId }> {
  projection: TabRegistryProjection<T>;
  needsSnapshot: boolean;
  removed?: { tabId: TabId; requested: boolean };
}

export interface TabRegistryRecovery<T extends { id: TabId }> {
  accept(event: TabRegistryEvent<T>): Promise<void> | null;
  recover(): Promise<void>;
  projection(): TabRegistryProjection<T>;
}

/** Apply one revisioned registry event without ever accepting a partial gap.
 *
 * Rust's bounded process-wide stream recovers its own overflow with a
 * snapshot. This second gate covers the renderer transport boundary: an event
 * listener installed late or a dropped Tauri event requests the same current
 * snapshot instead of constructing a roster from incomplete changes.
 */
export function applyTabRegistryEvent<T extends { id: TabId }>(
  projection: TabRegistryProjection<T>,
  event: TabRegistryEvent<T>,
): TabRegistryApplyResult<T> {
  const currentRevision = projection.revision;
  if (currentRevision !== null && event.revision <= currentRevision) {
    return { projection, needsSnapshot: false };
  }
  if (event.change !== "snapshot"
      && (currentRevision === null || event.revision !== currentRevision + 1)) {
    return { projection, needsSnapshot: true };
  }

  if (event.change === "snapshot") {
    const currentById = new Map(projection.tabs.map((tab) => [tab.id, tab]));
    return {
      projection: {
        revision: event.revision,
        tabs: event.tabs.map((tab) => ({
          ...currentById.get(tab.id),
          ...tab,
        } as T)),
      },
      needsSnapshot: false,
    };
  }

  if (event.change === "removed") {
    return {
      projection: {
        revision: event.revision,
        tabs: projection.tabs.filter((tab) => tab.id !== event.tabId),
      },
      needsSnapshot: false,
      removed: { tabId: event.tabId, requested: event.requested },
    };
  }

  const index = projection.tabs.findIndex((tab) => tab.id === event.tabId);
  if (event.change === "changed" && index === -1) {
    return { projection, needsSnapshot: true };
  }
  const next = projection.tabs.slice();
  const merged = { ...(index === -1 ? undefined : next[index]), ...event.tab } as T;
  if (index === -1) next.push(merged);
  else next[index] = merged;
  return {
    projection: { revision: event.revision, tabs: next },
    needsSnapshot: false,
  };
}

/** Keep the renderer lossless across the asynchronous snapshot boundary.
 *
 * Tauri events are ordered, but the invoke used to fetch a recovery snapshot
 * is an independent promise. Events received while that promise is pending
 * are retained here. Once snapshot R arrives, events through R are discarded,
 * contiguous events after R are replayed, and any gap causes another current
 * snapshot fetch instead of accepting partial state.
 */
export function createTabRegistryRecovery<T extends { id: TabId }>(
  initial: TabRegistryProjection<T>,
  loadSnapshot: () => Promise<{ revision: number; tabs: Array<Partial<T> & { id: TabId }> }>,
  onProjection: (projection: TabRegistryProjection<T>) => void,
): TabRegistryRecovery<T> {
  let current = initial;
  let pending: TabRegistryEvent<T>[] = [];
  let inFlight: Promise<void> | null = null;

  const commit = (event: TabRegistryEvent<T>): boolean => {
    const applied = applyTabRegistryEvent(current, event);
    if (applied.needsSnapshot) return false;
    if (applied.projection !== current) {
      current = applied.projection;
      onProjection(current);
    }
    return true;
  };

  const recover = (): Promise<void> => {
    if (inFlight !== null) return inFlight;
    inFlight = (async () => {
      for (;;) {
        const snapshot = await loadSnapshot();
        commit({ change: "snapshot", ...snapshot });

        const queued = pending;
        pending = [];
        let gap = false;
        for (let index = 0; index < queued.length; index += 1) {
          const event = queued[index];
          if (event.revision <= (current.revision ?? -1)) continue;
          if (!commit(event)) {
            pending.push(...queued.slice(index));
            gap = true;
            break;
          }
        }
        if (!gap) return;
      }
    })().finally(() => {
      inFlight = null;
    });
    return inFlight;
  };

  return {
    accept(event) {
      if (inFlight !== null) {
        pending.push(event);
        return inFlight;
      }
      if (!commit(event)) {
        pending.push(event);
        return recover();
      }
      return null;
    },
    recover,
    projection: () => current,
  };
}

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
