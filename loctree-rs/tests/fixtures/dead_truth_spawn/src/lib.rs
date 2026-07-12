//! Spawner side: references the bridge binary only by its string name —
//! the CodeScribe stt-bridge empiria (file "dead" in the graph, alive at
//! runtime via Command::new("stt-bridge")).

pub fn spawn_bridge() -> std::io::Result<std::process::Child> {
    std::process::Command::new("stt-bridge").spawn()
}
