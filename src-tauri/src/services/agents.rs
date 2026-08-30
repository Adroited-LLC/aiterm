//! Transport-independent agent discovery and launch resolution.

use crate::agents::{AgentChoice, Caps, Detection};
use crate::launch::{LaunchPlan, LaunchRequest};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct AgentService {
    operations: Arc<dyn AgentOperations>,
}

pub trait AgentOperations: Send + Sync + 'static {
    fn detect(&self) -> Vec<Detection>;
    fn caps(&self) -> HashMap<String, Caps>;
    fn list(&self) -> Vec<AgentChoice>;
    fn resolve(&self, request: LaunchRequest) -> Result<LaunchPlan, AgentServiceError>;
}

struct DesktopAgentOperations;
struct EmptyAgentOperations;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentServiceError {
    code: &'static str,
    message: String,
}

impl AgentServiceError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Default for AgentService {
    fn default() -> Self {
        Self::desktop()
    }
}

impl AgentService {
    pub fn desktop() -> Self {
        Self {
            operations: Arc::new(DesktopAgentOperations),
        }
    }

    /// A disabled service for gateway embeddings that expose no agents.
    pub fn empty() -> Self {
        Self {
            operations: Arc::new(EmptyAgentOperations),
        }
    }

    pub fn from_operations(operations: Arc<dyn AgentOperations>) -> Self {
        Self { operations }
    }

    pub fn detect(&self) -> Vec<Detection> {
        self.operations.detect()
    }

    pub fn caps(&self) -> HashMap<String, Caps> {
        self.operations.caps()
    }

    pub fn list(&self) -> Vec<AgentChoice> {
        self.operations.list()
    }

    pub fn resolve(&self, request: LaunchRequest) -> Result<LaunchPlan, AgentServiceError> {
        if let LaunchRequest::Agent {
            agent_id,
            model,
            effort,
            ..
        } = &request
        {
            let choices = self.operations.list();
            let choice = choices
                .iter()
                .find(|choice| choice.id == *agent_id)
                .ok_or_else(|| {
                    AgentServiceError::new(
                        "agent.unavailable",
                        "the requested agent is not offered and available",
                    )
                })?;
            let selected_model = match model {
                Some(model_id) => Some(
                    choice
                        .models
                        .iter()
                        .find(|candidate| candidate.id == *model_id)
                        .ok_or_else(|| {
                            AgentServiceError::new(
                                "agent.invalid_selection",
                                "the requested model is not offered for this agent",
                            )
                        })?,
                ),
                None => None,
            };
            if let Some(effort) = effort {
                let model = selected_model.ok_or_else(|| {
                    AgentServiceError::new(
                        "agent.invalid_selection",
                        "an effort requires an explicitly offered model",
                    )
                })?;
                if !model.efforts.iter().any(|candidate| candidate == effort) {
                    return Err(AgentServiceError::new(
                        "agent.invalid_selection",
                        "the requested effort is not offered for this model",
                    ));
                }
            }
        }
        self.operations.resolve(request)
    }
}

impl AgentOperations for DesktopAgentOperations {
    fn detect(&self) -> Vec<Detection> {
        crate::agents::backends()
            .iter()
            .map(|backend| backend.detect())
            .collect()
    }

    fn caps(&self) -> HashMap<String, Caps> {
        crate::agents::backends()
            .iter()
            .map(|backend| (backend.id().to_owned(), backend.caps()))
            .collect()
    }

    fn list(&self) -> Vec<AgentChoice> {
        crate::agents::backends()
            .iter()
            .filter(|backend| backend.offered() && backend.detect().available)
            .map(|backend| AgentChoice {
                id: backend.id().to_owned(),
                display_name: backend.display_name().to_owned(),
                models: backend.models(),
                mints_session_id: backend.mints_session_id(),
            })
            .collect()
    }

    fn resolve(&self, request: LaunchRequest) -> Result<LaunchPlan, AgentServiceError> {
        crate::launch::resolve_result(request)
            .map_err(|message| AgentServiceError::new("agent.unavailable", message))
    }
}

impl AgentOperations for EmptyAgentOperations {
    fn detect(&self) -> Vec<Detection> {
        Vec::new()
    }

    fn caps(&self) -> HashMap<String, Caps> {
        HashMap::new()
    }

    fn list(&self) -> Vec<AgentChoice> {
        Vec::new()
    }

    fn resolve(&self, _request: LaunchRequest) -> Result<LaunchPlan, AgentServiceError> {
        Err(AgentServiceError::new(
            "agent.unavailable",
            "the requested agent action is unavailable",
        ))
    }
}
