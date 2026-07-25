import ComposerPills from "./ComposerPills";

/**
 * The strip above the terminal.
 *
 * It used to be a text input with a status line under it. Both are gone: the
 * input duplicated the one claude already draws, and the status line repeated
 * what claude's own footer says (permission mode, branch) or described keys
 * that only mattered to the input it sat beneath. What is left is the part
 * that says something the terminal cannot — how much work is outstanding, what
 * has been written, what is running, and how much plan usage is left.
 *
 * Kept as a component rather than folded into App so the existing show/hide
 * toggle keeps working unchanged.
 */
export default function Composer({ sessionId }: { sessionId: string | null }) {
  return (
    <div className="composer-wrap">
      <ComposerPills sessionId={sessionId} />
    </div>
  );
}
