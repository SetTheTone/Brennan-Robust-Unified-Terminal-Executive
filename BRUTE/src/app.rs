use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
 
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = "invoke")]
    async fn invoke(cmd: &str, args: JsValue) -> JsValue;

    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "event"], js_name = "listen")]
    async fn listen(event: &str, handler: &Closure<dyn FnMut(JsValue)>) -> JsValue;
}

#[derive(Serialize)]
struct ConnectArgs {
    ip: String,
}

#[derive(Serialize)]
struct CommandArgs {
    command: String,
}

#[derive(Serialize)]
struct ScanArgs {
    #[serde(rename = "hintIp")]
    hint_ip: String,
}

#[derive(Serialize)]
struct SaveDownloadArgs {
    filename: String,
    content: String,
}

#[derive(Deserialize)]
struct TauriEvent {
    payload: String,
}

fn is_connect_success(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    !lower.contains("error")
        && !lower.contains("failed")
        && !lower.contains("refused")
        && !lower.contains("timed out")
}

/// Converts Linux ANSI escape sequences (from `ls --color=always`, etc.) into styled HTML spans
fn ansi_to_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 256);
    let mut chars = input.chars().peekable();
    let mut span_open = false;

    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            let mut code_str = String::new();
            let mut final_char = 'm';
            while let Some(&nc) = chars.peek() {
                chars.next();
                if nc.is_ascii_alphabetic() {
                    final_char = nc;
                    break;
                }
                code_str.push(nc);
            }

            if final_char == 'm' {
                if span_open {
                    out.push_str("</span>");
                    span_open = false;
                }

                let mut fg_color: Option<&str> = None;
                let mut bold = false;

                for part in code_str.split(';') {
                    match part {
                        "" | "0" | "00" => {
                            fg_color = None;
                            bold = false;
                        }
                        "1" | "01" => bold = true,
                        "31" | "91" => fg_color = Some("#f87171"), // Red: Archives / Broken links / Errors
                        "32" | "92" => fg_color = Some("#4ade80"), // Green: Executables / Scripts
                        "33" | "93" => fg_color = Some("#facc15"), // Yellow: Device nodes / FIFOs
                        "34" | "94" => fg_color = Some("#60a5fa"), // Blue: Directories
                        "35" | "95" => fg_color = Some("#c084fc"), // Magenta: Media / Audio / Sockets
                        "36" | "96" => fg_color = Some("#22d3ee"), // Cyan: Symbolic links
                        "37" | "97" => fg_color = Some("#f9fafb"), // White
                        _ => {}
                    }
                }

                if fg_color.is_some() || bold {
                    out.push_str("<span style=\"");
                    if let Some(col) = fg_color {
                        out.push_str("color:");
                        out.push_str(col);
                        out.push(';');
                    }
                    if bold {
                        out.push_str("font-weight:bold;");
                    }
                    out.push_str("\">");
                    span_open = true;
                }
            }
            continue;
        }

        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }

    if span_open {
        out.push_str("</span>");
    }

    out
}

fn format_brennan_json(raw: &str) -> String {
    let trimmed = raw.trim();

    let json_body = match (trimmed.find('{'), trimmed.rfind('}')) {
        (Some(start), Some(end)) if end > start => trimmed[start + 1..end].trim(),
        _ => trimmed
            .trim_end_matches('#')
            .trim()
            .trim_end_matches("/root")
            .trim()
            .trim_start_matches('{')
            .trim_end_matches('}')
            .trim(),
    };

    if json_body.is_empty() {
        return String::from("# ");
    }

    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut depth: usize = 0;
    let mut escape = false;

    for ch in json_body.chars() {
        if escape {
            current.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' && in_quotes {
            escape = true;
            continue;
        }
        if ch == '"' {
            in_quotes = !in_quotes;
            continue;
        }
        if !in_quotes {
            match ch {
                '[' | '{' => {
                    depth += 1;
                    current.push(ch);
                }
                ']' | '}' => {
                    depth = depth.saturating_sub(1);
                    current.push(ch);
                }
                ',' if depth == 0 => {
                    let item = current.trim().to_string();
                    if !item.is_empty() {
                        tokens.push(item);
                    }
                    current.clear();
                }
                _ => current.push(ch),
            }
        } else {
            current.push(ch);
        }
    }
    let last = current.trim().to_string();
    if !last.is_empty() {
        tokens.push(last);
    }

    let mut entries = Vec::new();
    let mut max_key_len = 0;

    for token in tokens {
        if let Some((k, v)) = token.split_once(':') {
            let key = k.trim().to_string();
            let val = v.trim().to_string();
            if key.len() > max_key_len {
                max_key_len = key.len();
            }
            entries.push((key, val));
        } else {
            entries.push((token, String::new()));
        }
    }

    let mut formatted_cells = Vec::new();
    let mut col_width = 36usize;

    for (k, v) in entries {
        let cell = if v.is_empty() {
            k
        } else {
            format!("{:<width$} : {}", k, v, width = max_key_len)
        };
        if cell.chars().count() + 4 > col_width {
            col_width = cell.chars().count() + 4;
        }
        formatted_cells.push(cell);
    }

    let mut out = String::new();
    for pair in formatted_cells.chunks(2) {
        if pair.len() == 2 {
            out.push_str(&format!(
                "{:<width$}{}\n",
                pair[0],
                pair[1],
                width = col_width
            ));
        } else {
            out.push_str(&format!("{}\n", pair[0]));
        }
    }

    out.push_str("# ");
    out
}

#[component]
pub fn App() -> impl IntoView {
    let (ip_input, set_ip_input) = signal(String::from("Example: 192.168.4.75"));
    let (ip_first_focus, set_ip_first_focus) = signal(true);
    let (cmd_input, set_cmd_input) = signal(String::new());
    let (connected, set_connected) = signal(false);
    let (terminal_text, set_terminal_text) = signal(String::from(
        "[BRUTE v1.0.0] Ready. Enter IP to connect...\n",
    ));

    let terminal_ref = NodeRef::<leptos::html::Pre>::new();

    // Auto-scroll whenever terminal_text changes
    Effect::new(move |_| {
        let _ = terminal_text.get();
        if let Some(el) = terminal_ref.get() {
            el.set_scroll_top(el.scroll_height());
        }
    });

    // Listen for live "scandisk-progress" events (used for both ScanDisk and Reboot progress)
    Effect::new(move |_| {
        spawn_local(async move {
            let closure = Closure::wrap(Box::new(move |event_js: JsValue| {
                if let Ok(ev) = serde_wasm_bindgen::from_value::<TauriEvent>(event_js) {
                    if ev.payload.contains("Auto-reconnect successful") {
                        set_connected.set(true);
                    }

                    set_terminal_text.update(|t| {
                        if ev.payload.starts_with("    --->") {
                            let trimmed = t.trim_end_matches('\n');
                            if let Some(last_nl) = trimmed.rfind('\n') {
                                if trimmed[last_nl + 1..].starts_with("    --->") {
                                    t.truncate(last_nl + 1);
                                }
                            }
                        }
                        t.push_str(&ev.payload);
                    });
                }
            }) as Box<dyn FnMut(JsValue)>);

            let _ = listen("scandisk-progress", &closure).await;
            closure.forget();
        });
    });

    let do_connect = move || {
        let ip = ip_input.get().trim().to_string();
        if ip.is_empty() || ip.starts_with("Example:") {
            set_terminal_text
                .update(|t| t.push_str("\n[Error: Please enter a valid IP Address]\n"));
            return;
        }
        set_ip_first_focus.set(false);
        set_terminal_text.update(|t| t.push_str(&format!("\n[Connecting to {}...]\n", ip)));

        spawn_local(async move {
            if let Ok(args) = serde_wasm_bindgen::to_value(&ConnectArgs { ip }) {
                let res = invoke("connect_ssh", args).await;
                if let Ok(msg) = serde_wasm_bindgen::from_value::<String>(res) {
                    if is_connect_success(&msg) {
                        set_connected.set(true);
                    }
                    set_terminal_text.update(|t| t.push_str(&msg));
                }
            }
        });
    };

    let handle_connect = move |_| {
        do_connect();
    };

    let handle_disconnect = move |_| {
        set_connected.set(false);
        spawn_local(async move {
            let res = invoke("disconnect_ssh", JsValue::NULL).await;
            if let Ok(msg) = serde_wasm_bindgen::from_value::<String>(res) {
                set_terminal_text.update(|t| t.push_str(&msg));
            }
        });
    };

    let handle_scan_subnet = move |_| {
        let hint_ip = ip_input.get().trim().to_string();
        set_terminal_text.update(|t| t.push_str("\n[Starting network scan to find Brennan...]\n"));
        spawn_local(async move {
            if let Ok(args) = serde_wasm_bindgen::to_value(&ScanArgs { hint_ip }) {
                let res = invoke("scan_subnet", args).await;
                if let Ok(found_ip) = serde_wasm_bindgen::from_value::<String>(res) {
                    if !found_ip.is_empty() && !found_ip.starts_with("Error:") {
                        set_ip_input.set(found_ip.clone());
                        set_ip_first_focus.set(false);
                        set_terminal_text.update(|t| {
                            t.push_str(&format!(
                                "\n[Success! Brennan found at: {} — Auto-connecting...]\n",
                                found_ip
                            ))
                        });

                        if let Ok(conn_args) =
                            serde_wasm_bindgen::to_value(&ConnectArgs { ip: found_ip })
                        {
                            let conn_res = invoke("connect_ssh", conn_args).await;
                            if let Ok(msg) = serde_wasm_bindgen::from_value::<String>(conn_res) {
                                if is_connect_success(&msg) {
                                    set_connected.set(true);
                                }
                                set_terminal_text.update(|t| t.push_str(&msg));
                            }
                        }
                    } else {
                        set_terminal_text
                            .update(|t| t.push_str("\n[No Brennan device found on subnet]\n"));
                    }
                } else {
                    set_terminal_text
                        .update(|t| t.push_str("\n[No Brennan device found on subnet]\n"));
                }
            }
        });
    };

    let run_cmd = move |cmd: &str| {
        let command = cmd.to_string();
        set_terminal_text.update(|t| t.push_str(&format!("{}\n", command)));
        spawn_local(async move {
            if let Ok(args) = serde_wasm_bindgen::to_value(&CommandArgs { command }) {
                let res = invoke("run_remote_command", args).await;
                if let Ok(msg) = serde_wasm_bindgen::from_value::<String>(res) {
                    set_terminal_text.update(|t| t.push_str(&msg));
                }
            }
        });
    };

    let handle_stats = move |_| {
        let command = String::from("wget -qO- http://127.0.0.1/b2cgi.fcgi?getWebInfo");
        set_terminal_text.update(|t| t.push_str("Stats\n"));
        spawn_local(async move {
            if let Ok(args) = serde_wasm_bindgen::to_value(&CommandArgs { command }) {
                let res = invoke("run_remote_command", args).await;
                if let Ok(msg) = serde_wasm_bindgen::from_value::<String>(res) {
                    let formatted = format_brennan_json(&msg);
                    set_terminal_text.update(|t| t.push_str(&formatted));
                }
            }
        });
    };

    let download_brennan_file = move |file_name: &'static str| {
        let ip = ip_input.get().trim().to_string();
        let url = format!("http://{}/media/hdd1/{}", ip, file_name);
        set_terminal_text.update(|t| {
            t.push_str(&format!(
                "[Downloading {} from {}...]\n",
                file_name, url
            ))
        });

        // Pass --color=never so the saved file header metadata stays plain text
        let ls_cmd = format!("ls --color=never -lt /media/hdd1/{}", file_name);
        let wget_cmd = format!("wget -qO- {}", url);

        spawn_local(async move {
            let mut ls_info = String::from("Unavailable");
            if let Ok(ls_args) = serde_wasm_bindgen::to_value(&CommandArgs { command: ls_cmd }) {
                let ls_res = invoke("run_remote_command", ls_args).await;
                if let Ok(raw_ls) = serde_wasm_bindgen::from_value::<String>(ls_res) {
                    let cleaned_ls = raw_ls.trim_end().trim_end_matches('#').trim();
                    if !cleaned_ls.is_empty() {
                        ls_info = cleaned_ls.to_string();
                    }
                }
            }

            if let Ok(args) = serde_wasm_bindgen::to_value(&CommandArgs { command: wget_cmd }) {
                let res = invoke("run_remote_command", args).await;
                if let Ok(raw_content) = serde_wasm_bindgen::from_value::<String>(res) {
                    let cleaned_content = raw_content
                        .trim_end()
                        .trim_end_matches('#')
                        .trim_end();

                    if cleaned_content.starts_with("Error:") || cleaned_content.is_empty() {
                        set_terminal_text.update(|t| {
                            t.push_str(&format!(
                                "[Error: Failed to retrieve {} from {}]\n{}\n# ",
                                file_name, url, cleaned_content
                            ))
                        });
                        return;
                    }

                    let now = String::from(
                        js_sys::Date::new_0().to_locale_string("en-GB", &JsValue::UNDEFINED),
                    );
                    let header = format!(
                        "========================================================\n\
                         File: {} (Source: {})\n\
                         File Info (ls -lt): {}\n\
                         Date and Time Generated: {}\n\
                         ========================================================\n\n",
                        file_name, url, ls_info, now
                    );
                    let footer = format!(
                        "\n\n========================================================\n\
                         End of {}\n\
                         File Info (ls -lt): {}\n\
                         Date and Time Generated: {}\n\
                         ========================================================\n",
                        file_name, ls_info, now
                    );

                    let full_content = format!("{}{}{}", header, cleaned_content, footer);

                    if let Ok(save_args) = serde_wasm_bindgen::to_value(&SaveDownloadArgs {
                        filename: file_name.to_string(),
                        content: full_content,
                    }) {
                        let save_res = invoke("save_to_downloads", save_args).await;
                        if let Ok(saved_path) = serde_wasm_bindgen::from_value::<String>(save_res) {
                            set_terminal_text.update(|t| {
                                t.push_str(&format!(
                                    "[Success: Saved to {} (Generated: {})]\n# ",
                                    saved_path, now
                                ))
                            });
                        }
                    }
                }
            }
        });
    };

    let handle_debug_log = move |_| {
        let ip = ip_input.get().trim().to_string();
        set_terminal_text.update(|t| {
            t.push_str("[Fetching full dmesg output and saving to DmesgLog...]\n")
        });

        let command = String::from("dmesg");
        spawn_local(async move {
            if let Ok(args) = serde_wasm_bindgen::to_value(&CommandArgs { command }) {
                let res = invoke("run_remote_command", args).await;
                if let Ok(raw_content) = serde_wasm_bindgen::from_value::<String>(res) {
                    let cleaned_content = raw_content
                        .trim_end()
                        .trim_end_matches('#')
                        .trim_end();

                    if cleaned_content.starts_with("Error:") || cleaned_content.is_empty() {
                        set_terminal_text.update(|t| {
                            t.push_str(&format!(
                                "[Error: Failed to retrieve dmesg from {}]\n{}\n# ",
                                ip, cleaned_content
                            ))
                        });
                        return;
                    }

                    let now = String::from(
                        js_sys::Date::new_0().to_locale_string("en-GB", &JsValue::UNDEFINED),
                    );
                    let header = format!(
                        "========================================================\n\
                         File: DmesgLog (Source: dmesg on {})\n\
                         Date and Time Generated: {}\n\
                         ========================================================\n\n",
                        ip, now
                    );
                    let footer = format!(
                        "\n\n========================================================\n\
                         End of DmesgLog\n\
                         Date and Time Generated: {}\n\
                         ========================================================\n",
                        now
                    );

                    let full_content = format!("{}{}{}", header, cleaned_content, footer);

                    if let Ok(save_args) = serde_wasm_bindgen::to_value(&SaveDownloadArgs {
                        filename: String::from("DmesgLog"),
                        content: full_content,
                    }) {
                        let save_res = invoke("save_to_downloads", save_args).await;
                        if let Ok(saved_path) = serde_wasm_bindgen::from_value::<String>(save_res) {
                            set_terminal_text.update(|t| {
                                t.push_str(&format!(
                                    "[Success: Saved to {} (Generated: {})]\n# ",
                                    saved_path, now
                                ))
                            });
                        }
                    }
                }
            }
        });
    };

    let handle_save_output = move |_| {
        let ip = ip_input.get().trim().to_string();
        let raw_terminal = terminal_text.get();
        let cleaned_content = raw_terminal.trim_end();

        let now = String::from(
            js_sys::Date::new_0().to_locale_string("en-GB", &JsValue::UNDEFINED),
        );
        let header = format!(
            "========================================================\n\
             File: TerminalOutput (Device IP: {})\n\
             Date and Time Generated: {}\n\
             ========================================================\n\n",
            ip, now
        );
        let footer = format!(
            "\n\n========================================================\n\
             End of TerminalOutput\n\
             Date and Time Generated: {}\n\
             ========================================================\n",
            now
        );

        let full_content = format!("{}{}{}", header, cleaned_content, footer);

        spawn_local(async move {
            if let Ok(save_args) = serde_wasm_bindgen::to_value(&SaveDownloadArgs {
                filename: String::from("TerminalOutput"),
                content: full_content,
            }) {
                let save_res = invoke("save_to_downloads", save_args).await;
                if let Ok(saved_path) = serde_wasm_bindgen::from_value::<String>(save_res) {
                    set_terminal_text.update(|t| {
                        t.push_str(&format!(
                            "\n[Success: Terminal output saved to {} (Generated: {})]\n# ",
                            saved_path, now
                        ))
                    });
                }
            }
        });
    };

    let handle_send_cmd = move |_| {
        let cmd = cmd_input.get().trim().to_string();
        if !cmd.is_empty() {
            set_cmd_input.set(String::new());
            run_cmd(&cmd);
        }
    };

    let handle_scandisk = move |_| {
        set_terminal_text.update(|t| t.push_str("--- Initiating ScanDisk ---\n"));
        spawn_local(async move {
            let _ = invoke("trigger_scandisk", JsValue::NULL).await;
        });
    };

    let handle_reboot = move |_| {
        set_terminal_text.update(|t| t.push_str("--- Initiating Reboot ---\n"));
        set_connected.set(false);
        spawn_local(async move {
            let _ = invoke("trigger_reboot", JsValue::NULL).await;
        });
    };

    let grid_btn_style = move || {
        if connected.get() {
            "background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;"
        } else {
            "background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: not-allowed; font-size: 1.0rem; opacity: 0.4;"
        }
    };

    view! {
        <main style="display: flex; flex-direction: column; width: 100vw; height: 100vh; margin: 0; padding: 12px 16px 16px 16px; box-sizing: border-box; overflow: hidden; background-color: #111827; color: #f3f4f6; font-family: sans-serif; border-top: none; border-left: 12px solid #22d3ee; border-right: 12px solid #22d3ee; border-bottom: 12px solid #22d3ee; border-radius: 0 0 6px 6px;">
            <div style="flex: 0 0 auto; padding-bottom: 12px; border-bottom: 1px solid #374151;">
                <div style="display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 10px; margin-bottom: 12px;">
                    <div>
                        <h1 style="margin: 0; font-size: 1.8rem; color: #22d3ee;">"BRUTE"</h1>
                        <p style="margin: 2px 0 0 0; font-size: 1.25rem; color: #27d300;">"Brennan Robust Unified Terminal Executive"</p>
                    </div>

                    <div style="display: flex; gap: 8px; align-items: center;">
                        <input
                            type="text"
                            placeholder="192.168.4.75"
                            prop:value=move || ip_input.get()
                            on:focus=move |_| {
                                if ip_first_focus.get() {
                                    set_ip_input.set(String::new());
                                    set_ip_first_focus.set(false);
                                }
                            }
                            on:input=move |ev| set_ip_input.set(event_target_value(&ev))
                            on:keydown=move |ev| {
                                if ev.key() == "Enter" && !connected.get() {
                                    do_connect();
                                }
                            }
                            disabled=move || connected.get()
                            style=move || if !connected.get() {
                                "background: #1f2937; color: #f3f4f6; border: 1px solid #4b5563; padding: 6px 10px; border-radius: 4px; width: 175px;"
                            } else {
                                "background: #1f2937; color: #f3f4f6; border: 1px solid #4b5563; padding: 6px 10px; border-radius: 4px; width: 175px; opacity: 0.4; cursor: not-allowed;"
                            }
                        />
                        <button
                            on:click=handle_connect
                            disabled=move || connected.get()
                            style=move || if !connected.get() {
                                "background: #0891b2; color: white; border: none; padding: 6px 12px; border-radius: 4px; cursor: pointer; font-weight: bold;"
                            } else {
                                "background: #0891b2; color: white; border: none; padding: 6px 12px; border-radius: 4px; cursor: not-allowed; font-weight: bold; opacity: 0.4;"
                            }
                        >
                            "Connect"
                        </button>
                        <button
                            on:click=handle_disconnect
                            disabled=move || !connected.get()
                            style=move || if connected.get() {
                                "background: #dc2626; color: white; border: none; padding: 6px 12px; border-radius: 4px; cursor: pointer; font-weight: bold;"
                            } else {
                                "background: #dc2626; color: white; border: none; padding: 6px 12px; border-radius: 4px; cursor: not-allowed; font-weight: bold; opacity: 0.4;"
                            }
                        >
                            "Disconnect"
                        </button>
                        <button
                            on:click=handle_scan_subnet
                            disabled=move || connected.get()
                            style=move || if !connected.get() {
                                "background: #4b5563; color: white; border: none; padding: 6px 12px; border-radius: 4px; cursor: pointer;"
                            } else {
                                "background: #4b5563; color: white; border: none; padding: 6px 12px; border-radius: 4px; cursor: not-allowed; opacity: 0.4;"
                            }
                        >
                            "Scan Subnet"
                        </button>
                    </div>
                </div>

                <div style="display: flex; justify-content: space-between; align-items: center; gap: 8px; margin-bottom: 12px;">
                    <div style="flex: 1; display: grid; grid-template-columns: repeat(auto-fill, minmax(105px, 1fr)); gap: 6px;">
                        <button disabled=move || !connected.get() style=grid_btn_style on:click=handle_reboot>"Reboot"</button>
                        <button disabled=move || !connected.get() style=grid_btn_style on:click=handle_scandisk>"Scan Disk"</button>
                        <button disabled=move || !connected.get() style=grid_btn_style on:click=handle_stats>"Stats"</button>
                        <button disabled=move || !connected.get() style=grid_btn_style on:click=move |_| run_cmd("df -h && fdisk -l")>"Drive Info"</button>
                        <button disabled=move || !connected.get() style=grid_btn_style on:click=move |_| run_cmd("iwconfig")>"Wi-Fi dB"</button>
                        <button disabled=move || !connected.get() style=grid_btn_style on:click=move |_| run_cmd("aplay -l")>"DAC Ver"</button>
                        <button disabled=move || !connected.get() style=grid_btn_style on:click=move |_| download_brennan_file("b2settings")>"b2Settings"</button>
                        <button disabled=move || !connected.get() style=grid_btn_style on:click=move |_| download_brennan_file("b2db")>"B2DB"</button>
                        <button disabled=move || !connected.get() style=grid_btn_style on:click=handle_debug_log>"Debug Log"</button>
                        <button style="background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;" on:click=handle_save_output>"Output"</button>
                    </div>

                    <button
                        style="background: #dc2626; color: white; border: none; padding: 6px 12px; border-radius: 4px; cursor: pointer; font-weight: bold; font-size: 1.0rem; white-space: nowrap;"
                        on:click=move |_| set_terminal_text.set(String::from("[Terminal Cleared]\n# "))
                    >
                        "Clear Output"
                    </button>
                </div>

                <div style="display: flex; gap: 8px;">
                    <input
                        type="text"
                        placeholder="Enter Linux command (e.g. ls -la, pwd, cd /media)..."
                        prop:value=move || cmd_input.get()
                        on:input=move |ev| set_cmd_input.set(event_target_value(&ev))
                        disabled=move || !connected.get()
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" && connected.get() {
                                handle_send_cmd(leptos::ev::MouseEvent::new("click").unwrap());
                            }
                        }
                        style=move || if connected.get() {
                            "flex: 1; background: #1f2937; color: #f3f4f6; border: 1px solid #4b5563; padding: 6px 10px; border-radius: 4px; font-family: monospace;"
                        } else {
                            "flex: 1; background: #1f2937; color: #f3f4f6; border: 1px solid #4b5563; padding: 6px 10px; border-radius: 4px; font-family: monospace; opacity: 0.4; cursor: not-allowed;"
                        }
                    />
                    <button
                        on:click=handle_send_cmd
                        disabled=move || !connected.get()
                        style=move || if connected.get() {
                            "background: #10b981; color: white; border: none; padding: 6px 16px; border-radius: 4px; cursor: pointer; font-weight: bold; font-size: 1.0rem; white-space: nowrap;"
                        } else {
                            "background: #10b981; color: white; border: none; padding: 6px 16px; border-radius: 4px; cursor: not-allowed; font-weight: bold; font-size: 1.0rem; white-space: nowrap; opacity: 0.4;"
                        }
                    >
                        "Send"
                    </button>
                </div>
            </div>

            <div style="flex: 1 1 auto; display: flex; flex-direction: column; min-height: 0; padding-top: 10px;">
                <pre
                    node_ref=terminal_ref
                    style="flex: 1 1 auto; margin: 0; background: #000000; color: #e5e7eb; font-family: monospace; font-size: 1.05rem; padding: 12px; border-radius: 6px; overflow-y: auto; border: 1px solid #1f2937; white-space: pre-wrap; word-break: break-word;"
                    inner_html=move || ansi_to_html(&terminal_text.get())
                ></pre>
            </div>

        </main>
    }
}
