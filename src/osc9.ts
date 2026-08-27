/** OSC 9 parsing, kept apart from the terminal component so it can be checked
 *  on its own — it is the one piece of the notification path with rules
 *  subtle enough to get wrong quietly. */

/** OSC 9;4 progress, as ConEmu defined it and Windows Terminal spread it.
 *  `state` 1 = a real percentage, 2 = error, 3 = indeterminate (busy, no
 *  number), 4 = paused. State 0 withdraws it and is reported as `null` rather
 *  than as a progress of zero, which means something quite different. */
export interface TermProgress {
  state: 1 | 2 | 3 | 4;
  /** Absent for indeterminate work. */
  pct: number | null;
}

/** What an OSC 9 body turned out to be. `null` for a body that is neither —
 *  an empty notification is not worth a badge. */
export type Osc9 =
  | { kind: "progress"; progress: TermProgress | null }
  | { kind: "message"; message: string }
  | null;

/** OSC 9 is two protocols wearing one number: `9;4;…` is ConEmu's progress
 *  report, anything else is an iTerm2-style notification whose body is the
 *  message. Claude enforces the same split from its end — it refuses an OSC 9
 *  body starting with a digit unless it is the 9;4 form — so keying on the
 *  prefix is a rule, not a guess.
 *
 *  Anything shaped like `4;…` but outside the defined states is treated as a
 *  message, because it is likelier to be text that happens to start that way
 *  than a progress report from a protocol that has no such state. */
export function parseOsc9(data: string): Osc9 {
  const progress = /^4;([0-4])(?:;(\d{1,3}))?$/.exec(data);
  if (progress) {
    const state = Number(progress[1]);
    if (state === 0) return { kind: "progress", progress: null };
    const pct = progress[2] === undefined ? null : Math.min(100, Number(progress[2]));
    return {
      kind: "progress",
      // Indeterminate carries no meaningful number even when one is sent.
      progress: { state: state as 1 | 2 | 3 | 4, pct: state === 3 ? null : pct },
    };
  }
  const message = data.trim();
  return message ? { kind: "message", message } : null;
}
