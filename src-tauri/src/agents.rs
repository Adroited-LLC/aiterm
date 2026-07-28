//! The agents aiterm knows how to work with.
//!
//! aiterm was built around Claude Code, and the assumption is spread thin
//! across the codebase rather than concentrated: the session store path, the
//! launch flags, the roster command and the transcript format are all just
//! *there*, in whatever function needed them. That is fine for one agent and
//! becomes untenable at two, because the second one does not announce itself —
//! it shows up as a session list that silently omits half your work, or a
//! search index that only covers one tool.
//!
//! This module is the one place that knows an agent exists. A backend answers
//! three questions, and they are deliberately separate:
//!
//! - **Who are you?** `id` and `display_name`. `id` is written onto every
//!   session the backend yields and is what the UI switches its icon on.
//! - **Are you here?** `detect`, which is cheap enough to call whenever the
//!   answer is wanted and never assumes a tool is installed.
//! - **Where are your sessions?** `sessions`, a [`SessionProvider`].
//!
//! ## What is *not* here yet, and why that matters
//!
//! Listing, indexing and transcript lookup route through this registry. A great
//! deal does not, and a second backend will find every one of these still
//! hard-wired to Claude Code:
//!
//! - **Liveness.** `read_roster` shells out to `claude agents --json`.
//! - **Lifecycle.** Resume, fork, stop and the `--session-id` mint in the UI
//!   all speak Claude Code's flags.
//! - **Panels.** Tasks, artifacts, agents and the model pills parse Claude
//!   Code's transcript records and read `~/.claude`.
//! - **Trash.** `session_delete` and restore know `~/.claude/projects` layout.
//!
//! Each of those is a real decision rather than a mechanical port — a CLI agent
//! has to be *asked* whether a session is alive, while an API-backed one knows
//! for free — so they are left explicit rather than hidden behind a trait that
//! pretends to abstract them. Adding a backend today gets you rows in the list
//! and hits in search. Everything else is still to do, and this comment is the
//! honest list of what.

use serde::Serialize;

use crate::sessions::{ClaudeProvider, Session, SessionProvider};

/// What is known about an agent on this machine right now.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Detection {
    pub id: String,
    pub display_name: String,
    /// Whether aiterm can actually use it. For a CLI agent this means the
    /// binary is on PATH; a future API-backed backend would report whether it
    /// has credentials.
    pub available: bool,
    /// First line of `<bin> --version`, when it answered. `None` covers both
    /// "not installed" and "installed but would not say", which are different
    /// facts — `available` is the one to branch on.
    pub version: Option<String>,
    /// Resolved binary path, so the UI can show *which* copy was found when
    /// several are installed.
    pub path: Option<String>,
}

pub trait AgentBackend: Send + Sync {
    /// Stable identifier. Written onto every session this backend yields, so
    /// changing it orphans the `agent` field on rows already in the index.
    fn id(&self) -> &'static str;

    /// Human-facing name, for settings and empty states.
    fn display_name(&self) -> &'static str;

    /// Is this agent usable on this machine?
    ///
    /// Called on demand rather than polled: availability changes when someone
    /// installs something, which is not something to spend a timer on. The
    /// PATH lookup is pure filesystem; only reading a version spawns anything,
    /// and only when the binary was found.
    fn detect(&self) -> Detection;

    /// Where this backend's sessions live.
    fn sessions(&self) -> &dyn SessionProvider;
}

pub struct ClaudeBackend;

impl AgentBackend for ClaudeBackend {
    fn id(&self) -> &'static str {
        "claude"
    }
    fn display_name(&self) -> &'static str {
        "Claude Code"
    }
    fn detect(&self) -> Detection {
        detect_cli(self.id(), self.display_name(), "claude")
    }
    fn sessions(&self) -> &dyn SessionProvider {
        &ClaudeProvider
    }
}

/// Every backend aiterm knows about.
///
/// One registry, so a new agent is added in one place and cannot be half-added
/// — the previous arrangement built this list inline in `list_sessions` while
/// the indexer named `ClaudeProvider` directly, which is exactly the shape that
/// gets you rows you cannot search.
pub fn backends() -> Vec<Box<dyn AgentBackend>> {
    vec![Box::new(ClaudeBackend)]
}

/// Every backend's sessions with their transcript paths, newest first.
///
/// The single entry point for "what sessions exist" — listing and indexing both
/// come through here, so a backend cannot be visible in one and absent from the
/// other.
///
/// Each row is stamped with the id of the backend that produced it, rather than
/// trusting the parser to label its own output. The parser is per-agent and its
/// label would be a second place for the name to live; this way `agent` cannot
/// disagree with the registry, which is what the UI switches on.
pub fn scan_all_with_paths() -> Vec<(Session, std::path::PathBuf)> {
    scan_backends(&backends())
}

/// The body of [`scan_all_with_paths`], over an explicit list.
///
/// Split out so the composition rules — tagging, global ordering — can be
/// tested against fake backends. With one real backend in the registry there is
/// otherwise nothing to compose, and the interesting behaviour would go
/// unexercised until the day a second one is added.
fn scan_backends(list: &[Box<dyn AgentBackend>]) -> Vec<(Session, std::path::PathBuf)> {
    let mut all: Vec<(Session, std::path::PathBuf)> = list
        .iter()
        .flat_map(|b| {
            let id = b.id();
            b.sessions()
                .scan_with_paths()
                .into_iter()
                .map(move |(mut s, path)| {
                    s.agent = id.to_string();
                    (s, path)
                })
        })
        .collect();
    // Sorted across all backends, not within each: the list is one timeline of
    // your work, and grouping it by which tool happened to produce a row would
    // be an odd thing to impose on it.
    all.sort_by(|a, b| b.0.last_active.cmp(&a.0.last_active));
    all
}

/// The transcript for `session_id`, from whichever backend owns it.
///
/// Ownership is decided by asking, not by inspecting the id: ids are opaque,
/// and a rule for telling one agent's from another's would be a guess that
/// breaks the first time a format changes. First backend to find the file wins,
/// which makes registry order the tie-break — see the id-collision test for why
/// that is stated rather than left to chance.
pub fn find_session_file_in(
    list: &[Box<dyn AgentBackend>],
    session_id: &str,
) -> Option<std::path::PathBuf> {
    list.iter()
        .find_map(|b| b.sessions().find_session_file(session_id))
}

/// What aiterm can see on this machine, in registry order.
///
/// Reports every known backend, present or not: "Codex — not installed" is
/// more useful in a settings panel than an absence, and it is the difference
/// between a tool aiterm does not support and one you have not installed.
#[tauri::command]
pub fn detect_agents() -> Vec<Detection> {
    backends().iter().map(|b| b.detect()).collect()
}

/// Resolve `bin` against PATH, the way a shell would.
///
/// Deliberately not `which`/`command -v`: spawning a shell to ask whether a
/// program exists costs more than the answer, and would make "is Codex
/// installed?" a process spawn per backend per call.
fn which(bin: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|candidate| is_executable_file(candidate))
}

#[cfg(unix)]
fn is_executable_file(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(p: &std::path::Path) -> bool {
    p.is_file()
}

/// Detection for a backend that is a command-line program.
///
/// A missing binary is the ordinary case, not an error: most machines will have
/// one of these agents and not the others, and that is worth showing plainly
/// rather than treating as a failure.
fn detect_cli(id: &str, display_name: &str, bin: &str) -> Detection {
    let found = which(bin);
    let version = found.as_ref().and_then(|p| read_version(p));
    Detection {
        id: id.to_string(),
        display_name: display_name.to_string(),
        available: found.is_some(),
        version,
        path: found.map(|p| p.to_string_lossy().into_owned()),
    }
}

/// First line of `<bin> --version`, or `None` if it failed or said nothing.
///
/// Some tools print their version to stderr, so both streams are considered.
/// A tool that is installed but will not report a version is still usable, so
/// this never affects `available`.
fn read_version(bin: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new(bin).arg("--version").output().ok()?;
    let text = if out.stdout.is_empty() { &out.stderr } else { &out.stdout };
    String::from_utf8_lossy(text)
        .lines()
        .next()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_finds_a_program_that_exists_and_misses_one_that_does_not() {
        assert!(which("sh").is_some(), "sh should be on PATH");
        assert!(
            which("definitely-not-a-real-binary-aiterm").is_none(),
            "invented a program that is not installed",
        );
    }

    #[test]
    fn which_ignores_directories_and_unexecutable_files() {
        // A directory named like the binary must not count as finding it.
        let dir = std::env::temp_dir().join("aiterm-which-test");
        let _ = std::fs::create_dir_all(dir.join("notabin"));
        assert!(!is_executable_file(&dir.join("notabin")), "a directory passed as executable");
    }

    /// The case that actually happens: an agent the user has not installed.
    /// It must report cleanly rather than erroring, because a settings panel
    /// showing "Codex — not installed" is the whole point.
    #[test]
    fn a_missing_cli_detects_as_unavailable_without_failing() {
        let d = detect_cli("ghost", "Ghost Agent", "definitely-not-a-real-binary-aiterm");
        assert!(!d.available);
        assert_eq!(d.version, None);
        assert_eq!(d.path, None);
        assert_eq!(d.display_name, "Ghost Agent");
    }

    /// `sh` stands in for an installed agent: present on PATH, with a resolved
    /// path. Whether it reports a `--version` is not asserted — some tools do
    /// not, which is exactly why `available` does not depend on it.
    #[test]
    fn an_installed_cli_detects_as_available_with_a_path() {
        let d = detect_cli("sh", "Bourne Shell", "sh");
        assert!(d.available, "sh was not detected");
        assert!(d.path.is_some_and(|p| p.ends_with("sh")));
    }

    #[test]
    fn every_backend_reports_its_own_identity() {
        for b in backends() {
            let d = b.detect();
            assert_eq!(d.id, b.id(), "detection reported a different id");
            assert_eq!(d.display_name, b.display_name());
            assert!(!d.id.is_empty() && !d.display_name.is_empty());
        }
    }

    /* ---- composition, against fake backends ----------------------------- */

    struct FakeProvider {
        /// (session id, last_active) — enough to test tagging and ordering.
        rows: Vec<(&'static str, u64)>,
    }

    impl SessionProvider for FakeProvider {
        fn scan_with_paths(&self) -> Vec<(Session, std::path::PathBuf)> {
            self.rows
                .iter()
                .map(|(id, at)| {
                    (
                        Session {
                            id: (*id).to_string(),
                            // Deliberately wrong: the registry must stamp this,
                            // not trust what the provider labelled it.
                            agent: "WRONG".into(),
                            title: (*id).to_string(),
                            project_path: "/p".into(),
                            group_path: "/p".into(),
                            branch: None,
                            forked: false,
                            background: false,
                            fork_parent: None,
                            last_active: *at,
                        },
                        std::path::PathBuf::from(format!("/fake/{id}")),
                    )
                })
                .collect()
        }
        fn find_session_file(&self, session_id: &str) -> Option<std::path::PathBuf> {
            self.rows
                .iter()
                .any(|(id, _)| *id == session_id)
                .then(|| std::path::PathBuf::from(format!("/fake/{session_id}")))
        }
    }

    struct FakeBackend {
        id: &'static str,
        provider: FakeProvider,
    }

    impl AgentBackend for FakeBackend {
        fn id(&self) -> &'static str {
            self.id
        }
        fn display_name(&self) -> &'static str {
            self.id
        }
        fn detect(&self) -> Detection {
            Detection {
                id: self.id.to_string(),
                display_name: self.id.to_string(),
                available: true,
                version: None,
                path: None,
            }
        }
        fn sessions(&self) -> &dyn SessionProvider {
            &self.provider
        }
    }

    fn fake(id: &'static str, rows: Vec<(&'static str, u64)>) -> Box<dyn AgentBackend> {
        Box::new(FakeBackend { id, provider: FakeProvider { rows } })
    }

    /// The whole point of the registry: two agents, one list.
    #[test]
    fn sessions_from_every_backend_appear_in_one_list() {
        let list = vec![fake("claude", vec![("c1", 10)]), fake("codex", vec![("x1", 20)])];
        let ids: Vec<String> = scan_backends(&list).into_iter().map(|(s, _)| s.id).collect();
        assert_eq!(ids.len(), 2, "a backend's sessions went missing");
        assert!(ids.contains(&"c1".to_string()) && ids.contains(&"x1".to_string()));
    }

    /// The registry is the source of truth for `agent`, not the parser. The UI
    /// switches its icon on this field, and a provider mislabelling its own
    /// output would be a second place for the name to live.
    #[test]
    fn every_row_is_tagged_with_the_backend_that_produced_it() {
        let list = vec![fake("claude", vec![("c1", 10)]), fake("codex", vec![("x1", 20)])];
        for (s, _) in scan_backends(&list) {
            let expected = if s.id == "c1" { "claude" } else { "codex" };
            assert_eq!(s.agent, expected, "row {} carried the wrong agent", s.id);
        }
    }

    /// One timeline of your work, not blocks grouped by tool. If ordering were
    /// per-backend, the newest session could sit below a week-old one purely
    /// because of which agent produced it.
    #[test]
    fn ordering_is_global_and_interleaves_backends() {
        let list = vec![
            fake("claude", vec![("old", 10), ("newest", 40)]),
            fake("codex", vec![("newer", 30), ("oldest", 5)]),
        ];
        let ids: Vec<String> = scan_backends(&list).into_iter().map(|(s, _)| s.id).collect();
        assert_eq!(ids, vec!["newest", "newer", "old", "oldest"]);
    }

    #[test]
    fn a_transcript_is_found_through_the_backend_that_owns_it() {
        let list = vec![fake("claude", vec![("c1", 10)]), fake("codex", vec![("x1", 20)])];
        assert_eq!(
            find_session_file_in(&list, "x1"),
            Some(std::path::PathBuf::from("/fake/x1")),
            "did not route to the owning backend",
        );
        assert_eq!(find_session_file_in(&list, "nobody"), None);
    }

    /// Ids are opaque and separately generated, so two agents *could* mint the
    /// same one. Nothing merges or dedupes them — both rows stay, each tagged
    /// with its own agent — and lookup resolves in registry order. Pinned here
    /// so the behaviour is a decision rather than an accident.
    #[test]
    fn colliding_ids_across_backends_stay_separate_rows() {
        let list = vec![fake("claude", vec![("same", 10)]), fake("codex", vec![("same", 20)])];
        let rows = scan_backends(&list);
        assert_eq!(rows.len(), 2, "rows from different agents were merged");
        let agents: Vec<String> = rows.into_iter().map(|(s, _)| s.agent).collect();
        assert!(agents.contains(&"claude".to_string()) && agents.contains(&"codex".to_string()));
        assert_eq!(
            find_session_file_in(&list, "same"),
            Some(std::path::PathBuf::from("/fake/same")),
            "lookup should resolve, first backend in registry order winning",
        );
    }

    #[test]
    fn an_empty_registry_is_not_an_error() {
        assert!(scan_backends(&[]).is_empty());
        assert_eq!(find_session_file_in(&[], "anything"), None);
    }

    #[test]
    fn backend_ids_are_unique() {
        let ids: Vec<&str> = backends().iter().map(|b| b.id()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "two backends share an id: {ids:?}");
    }
}
