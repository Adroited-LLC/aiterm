/** The desktop exit event emitted after a PTY's child has been reaped. */
export interface PtyExit {
  id: number;
  code: number | null;
  signal: string | null;
}

/**
 * Bridges the interval between installing the global desktop exit listener and
 * learning which numeric PTY id a spawn created. Events are indexed by id so
 * another tab's quick exit cannot displace this tab's event. `flush` waits
 * until the terminal has finished registering its handle before delivery.
 */
export function makePtyExitBuffer(deliver: (event: PtyExit) => void) {
  const pending = new Map<number, PtyExit>();
  let ptyId: number | null = null;
  let ready = false;
  let delivered = false;
  let disposed = false;

  const forward = (event: PtyExit) => {
    if (!disposed && !delivered && ready && event.id === ptyId) {
      delivered = true;
      deliver(event);
    }
  };

  return {
    receive(event: PtyExit) {
      if (disposed || delivered) return;
      if (ptyId === null || !ready) {
        pending.set(event.id, event);
        return;
      }
      forward(event);
    },

    bind(id: number) {
      if (!disposed) ptyId = id;
    },

    flush() {
      if (disposed || ptyId === null || delivered) return;
      ready = true;
      const event = pending.get(ptyId);
      pending.clear();
      if (event) forward(event);
    },

    dispose() {
      disposed = true;
      pending.clear();
    },
  };
}
