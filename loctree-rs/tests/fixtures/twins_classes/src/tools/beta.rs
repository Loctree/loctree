//! Registry pattern twin #2.

pub struct BetaTool;

impl BetaTool {
    pub fn register(&self, registry: &mut ToolRegistry) -> RegistrationOutcome {
        registry.add("beta");
        RegistrationOutcome::Ok
    }
}
