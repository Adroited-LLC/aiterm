# Sessions open in Codex Desktop

On Linux, AITerm identifies rollouts currently held by the Codex Desktop app-server using its `/proc` executable and open file descriptors. The session list shows **Open in Codex Desktop**. Resume and delete check ownership again and explain that the session must be closed there first. The transcript remains in the session list and is not hidden, moved, or taken over. Historical transcript origin alone does not block a session. Other deletion failures now appear in the UI, including bulk deletion.

The ownership label refreshes with the existing 30-second session refresh. Detection is best effort when process information is inaccessible; normal deletion leases remain the final protection against a competing process. This recognizes the Linux desktop bundle (`resources/codex`), not arbitrary external clients.

Session previews require a one-second hover. Pending previews cancel on row/window exit, blur, scroll, pointer press, keyboard input, and unmount. Dragging into a row does not arm a preview.

Validation: live work-PC inspection identified the test rollout held by `/usr/lib/chatgpt/resources/codex`; fixture tests verify desktop versus standalone CLI ownership and release; launch tests and the WSL companion check pass. Browser checks cover the label, hover delay, cancellation, drag suppression, and avoiding unnecessary transcript reads.

Linux package: 0.10.91. Prepared only: Matt requested no installation or desktop restart while his sessions are active.
