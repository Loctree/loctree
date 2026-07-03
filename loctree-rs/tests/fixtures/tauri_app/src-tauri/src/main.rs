// Backend Tauri handlers

#[tauri::command]
fn greet(name: String) -> String {
    format!("Hello, {}!", name)
}

#[tauri::command]
fn save_data(data: String) -> Result<(), String> {
    println!("Saving: {}", data);
    Ok(())
}

// This handler is defined but NOT called from frontend (unused)
#[tauri::command]
fn unused_handler() -> String {
    "I'm never called".to_string()
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![greet, save_data, unused_handler])
        .run(tauri::generate_context!())
        .expect("error");
}
