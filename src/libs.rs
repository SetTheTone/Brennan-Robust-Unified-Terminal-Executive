#[tauri::command]
fn connect_ssh(ip: String) -> String {
    // For now, let's return a confirmation string that we will stream to the console
    format!("Connecting to root@{} via SSH...\n", ip)
}
#[tauri::command]
fn disconnect_ssh() -> Result<String, String> {
    // If you maintain a global/managed state for the session later, we'll clear it here.
    // For now, this confirms the release signal to the frontend.
    Ok(String::from("\n--- IP Address Released & Disconnected ---\n"))
}

