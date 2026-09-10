# Tool events in the Android API view

## Audit

A read-only sample of 160 recent local Codex/Claude transcript files (up to the final 8 MB per file) contained 8,181 standalone external-agent wrapper messages: 4,090 calls, 3,962 normal results, and 129 error results. The format audit recognized all of these, including 41 empty results missed by the first paired-message-only parser. This is coverage of the sample, not a claim that future provider formats are known.

Observed external tools: Agent, Artifact, AskUserQuestion, Bash, Edit, Monitor, Read, SendMessage, Skill, TaskCreate, TaskStop, TaskUpdate, ToolSearch, WebFetch, WebSearch, Write.

The existing desktop conversation adapters already preserve arbitrary names for native Claude `tool_use`/`tool_result` and Codex `function_call`, `custom_tool_call`, and local-shell records. Normal MCP/plugin calls already become generic tool cards. The missing path was external agents whose tool operations had been serialized into assistant text.

## Rendering

Android 0.3.28 recognizes complete `[external_agent_tool_call: NAME]`, `[external_agent_tool_result]`, and `[external_agent_tool_result: error]` messages. Any tool name is accepted, including MCP/plugin names and future tools; known verbs choose an icon category and unfamiliar names retain a generic card. Results wrapped in `<tool_use_error>` expose the error text and failed status.

Calls/results may occupy one message or separate adjacent assistant messages. Without call IDs, only one unambiguous adjacent call is paired with a result. Parallel calls, user messages, prose, and turn boundaries prevent guessing. An unpaired result is its own result card; a call without a result is labeled Recorded, not successful or indefinitely running. Pairing preserves the call's display key as results arrive.

The native Activity group, loaded-history fallback, expanded subagent payloads, and Copy/Share use this rendering. Raw stored/wire messages are not rewritten. User examples, fenced code, incomplete records, and unrecognized envelope syntax remain visible instead of being silently discarded.

## Validation and rollout

Regression tests cover the reported Edit, all 16 observed tool names, unknown/MCP names, separate messages, errors, empty results, multiple calls, stable keys, conversation boundaries, Windows newlines, and copy/share content. The APK is prepared locally; no desktop update or installation is part of this change.
