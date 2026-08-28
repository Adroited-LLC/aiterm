/**
 * The small, deliberately conservative slice of terminal line editing needed
 * for lifecycle commands. It never guesses after an escape/control sequence
 * it does not model: a missed handoff is recoverable, assigning a terminal to
 * another conversation is not.
 */
export class TerminalInputLine {
  private text = "";
  private trustworthy = true;

  /** Record input. Returns the submitted line on Enter, `null` for a line
   * whose editing could not be represented safely, and `undefined` otherwise. */
  write(data: string): string | null | undefined {
    if (data === "\r" || data === "\n") {
      const submitted = this.trustworthy ? this.text : null;
      this.reset();
      return submitted;
    }
    if (data === "\x03") {
      this.reset();
      return undefined;
    }
    if (data === "\x7f" || data === "\b") {
      if (this.trustworthy) this.text = this.text.slice(0, -1);
      return undefined;
    }
    // Cursor movement, delete/word-delete, completion, and selection edits
    // can make an append-only representation lie about the submitted text.
    if (/[\x00-\x1f\x7f]/.test(data)) {
      this.trustworthy = false;
      return undefined;
    }
    if (this.trustworthy) this.text += data;
    return undefined;
  }

  paste(text: string) {
    this.write(text);
  }

  private reset() {
    this.text = "";
    this.trustworthy = true;
  }
}
