// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `aiterm --hook-report` is not the app: it is the one-shot helper a
    // Claude Code SessionStart hook runs to tell the running aiterm which
    // session just started in which process. Dispatch before Tauri exists —
    // it must cost milliseconds and never touch a display.
    if std::env::args().nth(1).as_deref() == Some(aiterm_lib::hooklink::HOOK_REPORT_FLAG) {
        aiterm_lib::hooklink::hook_report();
        return;
    }
    // `aiterm chat --provider X --model Y` is the console harness an
    // API-model tab runs — its own process inside the tab's PTY, exactly the
    // way `claude` is. Same rule: dispatch before Tauri exists.
    if std::env::args().nth(1).as_deref() == Some("chat") {
        let mut provider = None;
        let mut model = None;
        let mut session_id = None;
        let mut resume = None;
        let mut prompt = None;
        let mut args = std::env::args().skip(2);
        while let Some(a) = args.next() {
            match a.as_str() {
                "--provider" => provider = args.next(),
                "--model" => model = args.next(),
                "--session-id" => session_id = args.next(),
                "--resume" => resume = args.next(),
                "--prompt" => prompt = args.next(),
                _ => {}
            }
        }
        let start = match (resume, provider, model) {
            (Some(session_id), _, _) => aiterm_lib::chat::Start::Resume { session_id },
            (None, Some(provider_id), Some(model)) => aiterm_lib::chat::Start::Fresh {
                provider_id,
                model,
                session_id,
                prompt,
            },
            _ => {
                eprintln!(
                    "usage: aiterm chat --provider <id> --model <model-id> [--session-id <uuid>] [--prompt <text>]\n       aiterm chat --resume <uuid>"
                );
                std::process::exit(2);
            }
        };
        std::process::exit(aiterm_lib::chat::run(start));
    }
    // `aiterm mcp` is the stdio MCP server that exposes OpenCode delegation as a
    // tool to any Claude Code session that adds it. Like `chat`, it is its own
    // one-shot process that never touches a display — dispatch before Tauri.
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        std::process::exit(aiterm_lib::mcp::run());
    }
    aiterm_lib::run()
}
