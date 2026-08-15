use std::collections::BTreeMap;

use sdrmm_wire::{ToolDescriptor, ToolRequest, ToolResponse};

pub mod antenna;
pub mod nanovna;

pub use antenna::AntennaTool;
pub use nanovna::NanoVnaTool;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("no tool with id {0}")]
    Unknown(String),
    #[error("{0}")]
    Invalid(String),
    #[error("the {tool} tool cannot answer a {got} request")]
    WrongTool { tool: &'static str, got: String },
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

pub trait Tool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;

    fn run(&self, request: ToolRequest) -> Result<ToolResponse, ToolError>;
}

fn builtins() -> Vec<Box<dyn Tool>> {
    vec![Box::new(AntennaTool), Box::new(NanoVnaTool::default())]
}

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

    pub fn register(&mut self, tool: Box<dyn Tool>) -> Result<(), ToolError> {
        let id = tool.descriptor().id;
        if self.tools.contains_key(&id) {
            return Err(ToolError::DuplicateId(id));
        }
        self.tools.insert(id, tool);
        Ok(())
    }

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
    use std::sync::Arc;

    use sdrmm_wire::{
        AntennaDesign, AntennaRequest, NanoVnaDevice, NanoVnaMatch, NanoVnaRequest, NanoVnaResult,
        ToolCategory,
    };

    use super::*;

    struct Stub;

    struct NanoVnaBackendStub;

    impl nanovna::Backend for NanoVnaBackendStub {
        fn devices(&self) -> Result<(Vec<NanoVnaDevice>, Vec<String>), String> {
            Ok((
                vec![NanoVnaDevice {
                    port: "fixture-port".to_owned(),
                    label: "Fixture NanoVNA".to_owned(),
                    match_kind: NanoVnaMatch::Confirmed,
                    model: Some("NanoVNA-H4".to_owned()),
                    manufacturer: Some("nanovna.com".to_owned()),
                    product: Some("NanoVNA_H4".to_owned()),
                    serial_number: Some("fixture-serial".to_owned()),
                    usb_vid: Some(0x0483),
                    usb_pid: Some(0x5740),
                }],
                vec!["fixture-gnss".to_owned()],
            ))
        }

        fn connect(&self, _port: &str) -> Result<Box<dyn nanovna::Connection>, String> {
            Err("the discovery test must not connect".to_owned())
        }
    }

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
        assert!(descriptors.iter().any(|tool| tool.id == "nanovna"));
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

    #[test]
    fn the_registry_dispatches_nanovna_discovery() {
        let mut registry = ToolRegistry::default();
        registry
            .register(Box::new(NanoVnaTool::with_backend(Arc::new(
                NanoVnaBackendStub,
            ))))
            .expect("register fixture NanoVNA");
        let response = registry
            .run(ToolRequest::NanoVna(NanoVnaRequest::ListDevices))
            .expect("fixture discovery answers");
        let ToolResponse::NanoVna(result) = response else {
            panic!("the NanoVNA tool must answer under its own tag");
        };
        let NanoVnaResult::Devices {
            devices,
            ignored_ports,
        } = *result
        else {
            panic!("the NanoVNA tool must return device discovery");
        };
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].port, "fixture-port");
        assert_eq!(ignored_ports, vec!["fixture-gnss".to_owned()]);
    }
}
