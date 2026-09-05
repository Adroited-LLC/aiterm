use aiterm_lib::spine::{Kind, SpineEvent};
use serde::Serialize;
#[derive(Serialize)]
struct Page {
    epoch: u64,
    live: bool,
    has_more: bool,
    turn_open: bool,
    events: Vec<SpineEvent>,
}
fn main() {
    let page = Page {
        epoch: 1,
        live: true,
        has_more: false,
        turn_open: true,
        events: vec![SpineEvent {
            seq: 1,
            epoch: 1,
            session_id: "session-1".into(),
            agent: "codex".into(),
            ts: 9,
            kind: Kind::Phase {
                phase: aiterm_lib::spine::Phase::Working,
                detail: "running tests".into(),
            },
        }],
    };
    let mut bytes = Vec::new();
    ciborium::into_writer(&page, &mut bytes).unwrap();
    for b in bytes {
        print!("{b:02x}");
    }
    println!();
}
