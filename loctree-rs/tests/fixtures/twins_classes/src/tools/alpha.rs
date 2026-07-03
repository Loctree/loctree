//! Registry pattern: every tool implements `register` — name equality is the
//! contract, not duplication.

pub struct AlphaTool;

impl AlphaTool {
    pub fn register(&self, registry: &mut ToolRegistry) -> RegistrationOutcome {
        registry.add("alpha");
        RegistrationOutcome::Ok
    }
}
