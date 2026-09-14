# Android agent questions

Android 0.3.37 renders Codex `request_user_input` / `request_user_input_async`
and Claude `AskUserQuestion` as visible conversation cards outside collapsed
tool activity. Questions, option labels, descriptions, and multi-select hints
are preserved. Desktop 0.10.104 supplies up to 64 Ki characters of structured
question input instead of the ordinary 400-character tool summary. Oversized
or older truncated requests remain visible with an explanation and terminal
access; malformed JSON does not crash the conversation.

In a connected live session, tap **Answer in terminal** on a question card,
or **Session actions → Answer questions**. This opens the same terminal with
optional question controls. For Codex's deferred queue, tap **Open queued
questions** (Alt+Up), then use the arrows, Tab, Space, Enter, or the normal
composer for a written response. Blocking prompts may already be visible.
Hide closes only AiTerm's controls. Back returns to the API conversation.

Answers go through the existing authorized terminal attachment, preserving
the agent's actual question selection and acknowledgement. No choice is sent
automatically. This mode uses direct terminal input, bypassing normal prompt
receipt tracking so an answer does not leave a false pending-prompt card.
Controls are disabled without a connection and input ownership.

This is not a native Android answer-form RPC. Current AiTerm-launched Codex
processes are interactive terminals, not clients of a shared app-server
daemon. The generated Codex app-server schema exposes structured question
requests, but an independent app-server process cannot answer requests owned
by another running CLI. Transcript tool completion also does not establish
that an asynchronous question was answered: `accepted: true` only confirms
that the question was queued. Cards therefore do not invent a pending count
or mark accepted questions answered.

Validation covers six long questions, synchronous and asynchronous schemas,
Claude option descriptions, legacy/malformed input, activity grouping, and
cursor-mode key encoding. Device tests check visible question cards, explicit
navigation/submission taps, and read-only controls. A real queued Codex
question still requires user verification on the updated app.
