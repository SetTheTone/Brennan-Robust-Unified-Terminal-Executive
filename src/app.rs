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

#[derive(Deserialize)]
struct TauriEvent {
    payload: String,
}

#[component]
pub fn App() -> impl IntoView {
    let (ip_input, set_ip_input) = signal(String::from("Example: 192.168.4.75"));
    let (cmd_input, set_cmd_input) = signal(String::new());
    let (terminal_text, set_terminal_text) = signal(String::from(
        "[BRUTE v0.0.5] Ready. Enter IP to connect...\n",
    ));

    let terminal_ref = NodeRef::<leptos::html::Pre>::new();

    // Auto-scroll whenever terminal_text changes
    Effect::new(move |_| {
        let _ = terminal_text.get();
        if let Some(el) = terminal_ref.get() {
            el.set_scroll_top(el.scroll_height());
        }
    });

    // Listen for live "scandisk-progress" events emitted by the Rust backend
    // and overwrite the previous "   --->" progress line in-place
    Effect::new(move |_| {
        spawn_local(async move {
            let closure = Closure::wrap(Box::new(move |event_js: JsValue| {
                if let Ok(ev) = serde_wasm_bindgen::from_value::<TauriEvent>(event_js) {
                    set_terminal_text.update(|t| {
                        if ev.payload.starts_with("   --->") {
                            let trimmed = t.trim_end_matches('\n');
                            if let Some(last_nl) = trimmed.rfind('\n') {
                                if trimmed[last_nl + 1..].starts_with("   --->") {
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

    let handle_connect = move |_| {
        let ip = ip_input.get().trim().to_string();
        if ip.is_empty() {
            set_terminal_text.update(|t| t.push_str("\n[Error: Please enter a valid IP Address]\n"));
            return;
        }
        set_terminal_text.update(|t| t.push_str(&format!("\n[Connecting to {}...]\n", ip)));

        spawn_local(async move {
            if let Ok(args) = serde_wasm_bindgen::to_value(&ConnectArgs { ip }) {
                let res = invoke("connect_ssh", args).await;
                if let Ok(msg) = serde_wasm_bindgen::from_value::<String>(res) {
                    set_terminal_text.update(|t| t.push_str(&msg));
                }
            }
        });
    };

    let handle_disconnect = move |_| {
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
                        set_terminal_text.update(|t| {
                            t.push_str(&format!(
                                "\n[Success! Brennan found at: {} — Auto-connecting...]\n",
                                found_ip
                            ))
                        });

                        // Auto-connect immediately using the discovered IP
                        if let Ok(conn_args) =
                            serde_wasm_bindgen::to_value(&ConnectArgs { ip: found_ip })
                        {
                            let conn_res = invoke("connect_ssh", conn_args).await;
                            if let Ok(msg) = serde_wasm_bindgen::from_value::<String>(conn_res) {
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
        set_terminal_text.update(|t| t.push_str(&format!("\n$ {}\n", command)));
        spawn_local(async move {
            if let Ok(args) = serde_wasm_bindgen::to_value(&CommandArgs { command }) {
                let res = invoke("run_remote_command", args).await;
                if let Ok(msg) = serde_wasm_bindgen::from_value::<String>(res) {
                    set_terminal_text.update(|t| t.push_str(&msg));
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
        set_terminal_text.update(|t| t.push_str("\n--- Initiating ScanDisk ---\n"));
        spawn_local(async move {
            let _ = invoke("trigger_scandisk", JsValue::NULL).await;
        });
    };

    let handle_reboot = move |_| {
        set_terminal_text.update(|t| t.push_str("\n--- Initiating Reboot ---\n"));
        spawn_local(async move {
            let res = invoke("trigger_reboot", JsValue::NULL).await;
            if let Ok(err_msg) = serde_wasm_bindgen::from_value::<String>(res) {
                set_terminal_text.update(|t| t.push_str(&format!("[{}]\n", err_msg)));
            }
        });
    };

    view! {
        <main style="display: flex; flex-direction: column; height: 100vh; padding: 16px; box-sizing: border-box; overflow: hidden; background-color: #111827; color: #f3f4f6; font-family: sans-serif;">
            
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
                            on:input=move |ev| set_ip_input.set(event_target_value(&ev))
                            style="background: #1f2937; color: #f3f4f6; border: 1px solid #4b5563; padding: 6px 10px; border-radius: 4px; width: 175px;"
                        />
                        <button
                            on:click=handle_connect
                            style="background: #0891b2; color: white; border: none; padding: 6px 12px; border-radius: 4px; cursor: pointer; font-weight: bold;"
                        >
                            "Connect"
                        </button>
                        <button
                            on:click=handle_disconnect
                            style="background: #dc2626; color: white; border: none; padding: 6px 12px; border-radius: 4px; cursor: pointer; font-weight: bold;"
                        >
                            "Disconnect"
                        </button>
                        <button
                            on:click=handle_scan_subnet
                            style="background: #4b5563; color: white; border: none; padding: 6px 12px; border-radius: 4px; cursor: pointer;"
                        >
                            "Scan Subnet"
                        </button>
                    </div>
                </div>

                <div style="display: grid; grid-template-columns: repeat(auto-fill, minmax(110px, 1fr)); gap: 6px; margin-bottom: 12px;">
                    <button style="background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;" on:click=handle_reboot>"Reboot"</button>
                    <button style="background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;" on:click=handle_scandisk>"Scan Disk"</button>
                    <button style="background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;" on:click=move |_| run_cmd("wget -qO- http://127.0.0.1/b2cgi.fcgi?status")>"Scan Sonos"</button>
                    <button style="background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;" on:click=move |_| run_cmd("rm -f /b2db/sonos*")>"Forget Sonos"</button>
                    <button style="background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;" on:click=move |_| run_cmd("wget -qO- http://127.0.0.1/b2cgi.fcgi?getWebInfo")>"Version"</button>
                    <button style="background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;" on:click=move |_| run_cmd("wget -qO- http://127.0.0.1/b2cgi.fcgi?getWebInfo")>"Stats"</button>
                    <button style="background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;" on:click=move |_| run_cmd("df -h && fdisk -l")>"Drive Info"</button>
                    <button style="background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;" on:click=move |_| run_cmd("cat /proc/net/wireless")>"Wi-Fi dB"</button>
                    <button style="background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;" on:click=move |_| run_cmd("aplay -l")>"DAC Ver"</button>
                    <button style="background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;" on:click=move |_| run_cmd("dmesg | tail -n 50")>"Debug Log"</button>
                    <button style="background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;" on:click=move |_| run_cmd("cat /b2db/b2settings")>"b2Settings"</button>
                    <button style="background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;" on:click=move |_| run_cmd("head -n 50 /b2db/b2db")>"B2DB"</button>
                    <button style="background: #1f2937; color: #e5e7eb; border: 1px solid #374151; padding: 6px; border-radius: 4px; cursor: pointer; font-size: 1.0rem;" on:click=move |_| set_terminal_text.set(String::from("[Terminal Cleared]\n"))>"Clear Output"</button>
                </div>

                <div style="display: flex; gap: 8px;">
                    <input
                        type="text"
                        placeholder="Enter Linux command (e.g. ls -la, pwd, cd /media)..."
                        prop:value=move || cmd_input.get()
                        on:input=move |ev| set_cmd_input.set(event_target_value(&ev))
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" {
                                handle_send_cmd(leptos::ev::MouseEvent::new("click").unwrap());
                            }
                        }
                        style="flex: 1; background: #1f2937; color: #f3f4f6; border: 1px solid #4b5563; padding: 6px 10px; border-radius: 4px; font-family: monospace;"
                    />
                    <button
                        on:click=handle_send_cmd
                        style="background: #10b981; color: white; border: none; padding: 6px 16px; border-radius: 4px; cursor: pointer; font-weight: bold;"
                    >
                        "Send"
                    </button>
                </div>
            </div>

            <div style="flex: 1 1 auto; display: flex; flex-direction: column; min-height: 0; padding-top: 10px;">
                <pre
                    node_ref=terminal_ref
                    style="flex: 1 1 auto; margin: 0; background: #000000; color: #4ade80; font-family: monospace; font-size: 1.05rem; padding: 12px; border-radius: 6px; overflow-y: auto; border: 1px solid #1f2937; white-space: pre-wrap; word-break: break-word;"
                >
                    {move || terminal_text.get()}
                </pre>
            </div>

        </main>
    }
}
