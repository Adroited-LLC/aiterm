/** Ordered, coalesced writes to a pty.
 *
 * Two jobs, both of which exist because `pty_write` became an async command.
 *
 * **Order.** A sync Tauri command ran on the main thread, so writes happened in
 * the order they were invoked. An async one is a task on the runtime, and
 * nothing promises two tasks are polled in the order they were spawned — so
 * typing quickly could, in principle, deliver `acb`. Scrambled keystrokes are a
 * far worse bug than the lag the async move was fixing. Here only one write per
 * pty is ever in flight, so the runtime is never given two to race.
 *
 * **Volume.** Everything typed while a write is in flight merges into the next
 * one, so a fast burst or a paste crosses the IPC boundary once instead of per
 * character. Fewer round trips is the same win as the async move, from the
 * other end.
 *
 * `send` is injected so the queue can be tested without Tauri.
 */
export function makeWriteQueue(send: (id: number, data: string) => Promise<void>) {
  /** Bytes waiting to go out, per pty. */
  const outbox = new Map<number, string>();
  /** The drain in flight, per pty. Its presence is what stops a second one. */
  const inflight = new Map<number, Promise<void>>();

  async function drain(id: number): Promise<void> {
    try {
      for (;;) {
        const pending = outbox.get(id);
        if (!pending) return;
        // Taken before the await: anything typed during the send lands in a
        // fresh entry and goes out on the next turn of this loop, still in
        // order, without being lost.
        outbox.delete(id);
        await send(id, pending);
      }
    } finally {
      inflight.delete(id);
    }
  }

  return (id: number, data: string): Promise<void> => {
    if (!data) return Promise.resolve();
    outbox.set(id, (outbox.get(id) ?? "") + data);
    const running = inflight.get(id);
    if (running) return running;
    const chain = drain(id);
    inflight.set(id, chain);
    return chain;
  };
}
