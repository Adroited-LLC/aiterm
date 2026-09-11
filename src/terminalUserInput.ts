import type { Terminal, IDisposable } from "@xterm/xterm";

/** xterm's public onData also includes parser replies and focus reports. Its
 * synchronous user-input event distinguishes typing, paste and IME from those
 * replies. Keep this narrow internal adapter covered against our xterm version. */
export function onTerminalInput(term: Terminal, receive: (data: string, user: boolean) => void): IDisposable {
  const core = (term as unknown as { _core: { coreService: { onUserInput: (fn: () => void) => IDisposable } } })._core.coreService;
  let user = false;
  const input = core.onUserInput(() => { user = true; });
  const data = term.onData(value => { const typed = user; user = false; receive(value, typed); });
  return { dispose() { input.dispose(); data.dispose(); } };
}
