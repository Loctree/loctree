#[test]
fn context_handler_does_not_redeclare_pack_contract_symbols() {
    let handler = include_str!("../src/cli/dispatch/handlers/context/mod.rs");
    let forbidden = [
        "CONTEXT_SCHEMA_VERSION",
        "AuthorityLabel",
        "ContextPack",
        "ProjectIdentity",
        "StructuralRole",
        "StructuralFile",
        "StructuralSymbol",
        "StructuralImport",
        "ConsumerKind",
        "StructuralConsumer",
        "StructuralEntrypoint",
        "StructuralSlice",
        "RuntimeSlice",
        "RuntimeIdiomTag",
        "RuntimeDispatchEdge",
        "RuntimeReachability",
        "RuntimeEnvContract",
        "RuntimeTauriCommand",
        "RuntimeTauriEvent",
        "RuntimeFrameworkHint",
        "RiskSlice",
        "HotspotFile",
        "HighFanInFile",
        "RiskCacheScope",
        "ActionSlice",
        "MemoryEntry",
        "MemorySlice",
        "AuthoritySlice",
        "compose_context_pack",
        "compose_context_pack_from_snapshot",
        "compose_structural_slice",
        "compose_runtime_slice",
        "compose_risk_slice",
        "compose_action_slice",
        "compose_memory_slice",
    ];

    for symbol in forbidden {
        for prefix in [
            "pub const ",
            "pub struct ",
            "pub enum ",
            "pub fn ",
            "pub(crate) fn ",
        ] {
            let needle = format!("{prefix}{symbol}");
            assert!(
                !handler.contains(&needle),
                "context handler must not redeclare pack contract symbol `{symbol}`"
            );
        }
    }
}

#[test]
fn pack_does_not_import_cli_handlers() {
    let pack = include_str!("../src/pack.rs");

    assert!(
        !pack.contains("crate::cli::dispatch::handlers"),
        "pack.rs must not import CLI handler modules"
    );
}
