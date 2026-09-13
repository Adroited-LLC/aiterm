//! Read-only session status projection from the same atomic spine pages Android uses.
use serde_json::Value;

#[derive(Default)]
pub(super) struct SessionStatus {
    pub epoch: u64,
    pub latest: u64,
    phase: String,
    detail: String,
    live: bool,
    open: Option<bool>,
}
impl SessionStatus {
    pub fn apply(&mut self, page: &Value) {
        let epoch = page["epoch"].as_u64().unwrap_or(0);
        let latest = page["latest_seq"].as_u64().unwrap_or(0);
        if self.epoch != epoch {
            *self = Self {
                epoch,
                ..Self::default()
            };
        }
        if latest < self.latest {
            return;
        }
        let open = page["turn_open"].as_bool();
        if self.open != open {
            self.phase.clear();
            self.detail.clear();
        }
        if page["has_more"] == false {
            if let Some(events) = page["events"].as_array() {
                for event in events {
                    match event["kind"].as_str() {
                        Some("turn_started" | "turn_ended" | "reset") => {
                            self.phase.clear();
                            self.detail.clear();
                        }
                        Some("phase") => {
                            self.phase = event["phase"].as_str().unwrap_or("").into();
                            self.detail = event["detail"].as_str().unwrap_or("").into();
                        }
                        _ => {}
                    }
                }
            }
        }
        self.latest = latest;
        self.live = page["live"] == true;
        self.open = open;
    }
    pub fn activity(&self) -> Option<&'static str> {
        if !self.live {
            return None;
        }
        match self.open {
            Some(false) => Some("idle"),
            Some(true)
                if self.phase == "needs_you"
                    && !matches!(self.detail.as_str(), "approval" | "a tool call is waiting") =>
            {
                Some("attention")
            }
            Some(true) => Some("output"),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn turn_boundaries_outrank_repainting_and_inferred_attention() {
        let mut s = SessionStatus::default();
        s.apply(&json!({"epoch":1,"latest_seq":1,"live":true,"turn_open":true,"has_more":false,"events":[{"kind":"phase","phase":"needs_you","detail":"approval"}]}));
        assert_eq!(s.activity(), Some("output"));
        s.apply(&json!({"epoch":1,"latest_seq":2,"live":true,"turn_open":true,"has_more":false,"events":[{"kind":"phase","phase":"needs_you","detail":"question"}]}));
        assert_eq!(s.activity(), Some("attention"));
        s.apply(&json!({"epoch":1,"latest_seq":3,"live":true,"turn_open":false,"has_more":false,"events":[]}));
        assert_eq!(s.activity(), Some("idle"));
        s.apply(&json!({"epoch":1,"latest_seq":2,"live":true,"turn_open":true,"has_more":false,"events":[]}));
        assert_eq!(s.activity(), Some("idle"));
        s.apply(&json!({"epoch":2,"latest_seq":0,"live":false,"turn_open":null,"has_more":false,"events":[]}));
        assert_eq!(s.activity(), None);
    }
}
