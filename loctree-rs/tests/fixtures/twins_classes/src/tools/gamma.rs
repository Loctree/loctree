//! Registry pattern twin #3.

pub struct GammaTool;

impl GammaTool {
    pub fn register(&self, registry: &mut ToolRegistry) -> RegistrationOutcome {
        registry.add("gamma");
        RegistrationOutcome::Ok
    }
}
