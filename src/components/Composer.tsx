import ComposerPills from "./ComposerPills";
import { PermissionMode } from "../term/screen";
import { UsageBar } from "../ipc";

/**
 * The strip above the terminal.
 *
 * It used to be a text input with a status line under it. Both are gone: the
 * input duplicated the one claude already draws, and the status line repeated
 * what claude's own footer says (permission mode, branch) or described keys
 * that only mattered to the input it sat beneath. What is left is the part
 * that says something the terminal cannot — how much work is outstanding, what
 * has been written, what is running, how much plan usage is left, and which
 * model and effort the session is actually running at.
 *
 * Kept as a component rather than folded into App so the existing show/hide
 * toggle keeps working unchanged.
 */
export default function Composer({
  sessionId, projectRoot, usage, usageAsOf, onCommand, onDismiss, hasPendingInput, onOpenModelPicker,
  onOpenRewind, permMode, onSetPermMode,
}: {
  sessionId: string | null;
  projectRoot: string | null;
  usage: UsageBar[];
  /** When the shown usage was read, if it is a cached reading not yet
   *  refreshed this run. null once a live call has succeeded. */
  usageAsOf: number | null;
  onCommand?: (text: string) => void;
  onDismiss?: () => void;
  hasPendingInput?: () => boolean;
  onOpenModelPicker?: () => void;
  onOpenRewind?: () => void;
  permMode: PermissionMode | null;
  onSetPermMode?: (m: PermissionMode) => Promise<void>;
}) {
  return (
    <div className="composer-wrap">
      <ComposerPills
        sessionId={sessionId}
        projectRoot={projectRoot}
        usage={usage}
        usageAsOf={usageAsOf}
        onCommand={onCommand}
        onDismiss={onDismiss}
        hasPendingInput={hasPendingInput}
        onOpenModelPicker={onOpenModelPicker}
        onOpenRewind={onOpenRewind}
        permMode={permMode}
        onSetPermMode={onSetPermMode}
      />
    </div>
  );
}
