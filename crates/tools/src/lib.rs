//! The tool plane: instruments and calculators that stand beside the receiver.
//!
//! Nothing here touches a device, a channel or the DSP graph, and nothing in the signal path
//! may depend on this crate. A tool is a blocking request/response function — the server runs
//! it off the async executor — so a tool that talks to a serial instrument can simply block on
//! the port instead of colouring the whole framework async.
//!
//! Adding a tool: a variant on [`sdrmm_wire::ToolRequest`] and [`sdrmm_wire::ToolResponse`], a
//! module here implementing [`Tool`], and an entry in [`builtins`].

use std::collections::BTreeMap;

use sdrmm_wire::{ToolDescriptor, ToolRequest, ToolResponse};

pub mod antenna;

pub use antenna::AntennaTool;

/// Why a tool call did not produce an answer.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("no tool with id {0}")]
    Unknown(String),
    #[error("{0}")]
    Invalid(String),
    /// The registry handed a tool a request belonging to another one. Unreachable through the
    /// registry's own dispatch; it exists so a tool never has to guess.
    #[error("the {tool} tool cannot answer a {got} request")]
    WrongTool { tool: &'static str, got: String },
    /// Compiled in, but the hardware or resource it needs is not there.
    #[error("the {tool} tool is unavailable: {reason}")]
    Unavailable { tool: &'static str, reason: String },
    #[error("the {tool} tool failed: {reason}")]
    Failed { tool: &'static str, reason: String },
    #[error("a tool with id {0} is already registered")]
    DuplicateId(String),
}

impl ToolError {
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Unknown(_))
    }

    #[must_use]
    pub fn is_bad_request(&self) -> bool {
        matches!(self, Self::Invalid(_) | Self::WrongTool { .. })
    }

    #[must_use]
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable { .. })
    }
}

/// One tool. Implementors are shared across threads and hold whatever state they need behind
/// their own synchronisation; [`Tool::run`] takes `&self` so the registry can be an `Arc`.
pub trait Tool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;

    /// Answer one request. May block: the caller is responsible for keeping it off an async
    /// executor.
    fn run(&self, request: ToolRequest) -> Result<ToolResponse, ToolError>;
}

/// Every tool this build has. Feature-gated ones are absent rather than reported as broken.
fn builtins() -> Vec<Box<dyn Tool>> {
    vec![Box::new(AntennaTool)]
}

/// The tools a server offers, keyed by the id their requests are tagged with.
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn with_builtins() -> Self {
        let mut registry = Self::default();
        for tool in builtins() {
            registry.tools.insert(tool.descriptor().id, tool);
        }
        registry
    }

    /// Add a tool that is not a builtin — a desktop-only instrument, or a stub in a test.
    ///
    /// # Errors
    /// [`ToolError::DuplicateId`] if the id is taken: a silent replacement would leave the
    /// launcher advertising one tool and the router running another.
    pub fn register(&mut self, tool: Box<dyn Tool>) -> Result<(), ToolError> {
        let id = tool.descriptor().id;
        if self.tools.contains_key(&id) {
            return Err(ToolError::DuplicateId(id));
        }
        self.tools.insert(id, tool);
        Ok(())
    }

    /// What `GET /api/tools` lists, ordered by name so the launcher does not have to sort.
    #[must_use]
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        let mut descriptors: Vec<ToolDescriptor> =
            self.tools.values().map(|tool| tool.descriptor()).collect();
        descriptors.sort_by(|left, right| left.name.cmp(&right.name));
        descriptors
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&dyn Tool> {
        self.tools.get(id).map(AsRef::as_ref)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Run a request against the tool its tag names.
    ///
    /// # Errors
    /// [`ToolError::Unknown`] if no tool answers to the tag, otherwise whatever the tool
    /// itself refused or failed with.
    pub fn run(&self, request: ToolRequest) -> Result<ToolResponse, ToolError> {
        let id = request.tool_id();
        let tool = self
            .tools
            .get(id)
            .ok_or_else(|| ToolError::Unknown(id.to_owned()))?;
        tool.run(request)
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{AntennaDesign, AntennaRequest, ToolCategory};

    use super::*;

    struct Stub;

    impl Tool for Stub {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                id: "antenna".to_owned(),
                name: "Stub".to_owned(),
                summary: "duplicate of a builtin".to_owned(),
                category: ToolCategory::Calculator,
                needs_hardware: false,
            }
        }

        fn run(&self, _request: ToolRequest) -> Result<ToolResponse, ToolError> {
            Err(ToolError::Failed {
                tool: "stub",
                reason: "never runs".to_owned(),
            })
        }
    }

    #[test]
    fn every_builtin_is_registered_and_listed_once() {
        let registry = ToolRegistry::with_builtins();
        assert_eq!(registry.len(), builtins().len());
        let descriptors = registry.descriptors();
        assert_eq!(descriptors.len(), builtins().len());
        assert!(descriptors.iter().any(|tool| tool.id == "antenna"));
    }

    #[test]
    fn registering_a_taken_id_is_refused_rather_than_replacing_the_tool() {
        let mut registry = ToolRegistry::with_builtins();
        let err = registry
            .register(Box::new(Stub))
            .expect_err("the antenna id is taken");
        assert!(matches!(err, ToolError::DuplicateId(id) if id == "antenna"));
        assert_eq!(
            registry
                .get("antenna")
                .expect("still the builtin")
                .descriptor()
                .name,
            "Antenna calculator"
        );
    }

    #[test]
    fn an_empty_registry_answers_no_request() {
        let registry = ToolRegistry::default();
        assert!(registry.is_empty());
        let err = registry
            .run(ToolRequest::Antenna(AntennaRequest::default()))
            .expect_err("nothing registered");
        assert!(err.is_not_found(), "{err}");
    }

    #[test]
    fn the_registry_dispatches_on_the_request_tag() {
        let registry = ToolRegistry::with_builtins();
        let response = registry
            .run(ToolRequest::Antenna(AntennaRequest {
                frequency_hz: 145_500_000.0,
                design: AntennaDesign::Dipole,
                ..AntennaRequest::default()
            }))
            .expect("the antenna tool answers");
        assert_eq!(response.tool_id(), "antenna");
    }
}
