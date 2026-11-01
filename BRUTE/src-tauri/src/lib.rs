use ssh2::Session;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::Manager;

struct SshState {
    session: Mutex<Option<Session>>,
    current_dir: Mutex<String>,
    ip: Mutex<String>,
}

/// Masks a string between its first and last characters with '*'
fn mask_middle(val: &str) -> String {
    let trimmed = val.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    match chars.len() {
        0 => String::new(),
        1 => "*".to_string(),
        2 => format!("{}*", chars[0]),
        n => {
            let first = chars[0];
            let last = chars[n - 1];
            let stars = "*".repeat(n - 2);
            format!("{}{}{}", first, stars, last)
        }
    }
}

/// Masks the WiFi password in b2settings content while keeping first and last char
fn mask_b2settings_wifi_password(content: &str) -> String {
    let mut result = Vec::new();
    let mut mask_next_line = false;

    for line in content.lines() {
        if mask_next_line {
            if !line.trim().is_empty() {
                result.push(mask_middle(line));
            } else {
                result.push(line.to_string());
            }
            mask_next_line = false;
            continue;
        }

        let lower = line.trim().to_lowercase();

        // Brennan b2settings format often has "Wifi SSID" followed by a password key, or "psk" / "pass" / "key"
        if (lower.contains("wifi") && (lower.contains("pass") || lower.contains("key") || lower.contains("psk") || lower.contains("pwd")))
            || lower.starts_with("psk")
            || lower.starts_with("passphrase")
            || lower.starts_with("password")
            || lower.starts_with("wifi_pass")
            || lower.starts_with("wifipass")
        {
            if let Some((k, v)) = line.split_once(':') {
                if !v.trim().is_empty() {
                    result.push(format!("{}: {}", k, mask_middle(v)));
                    continue;
                } else {
                    mask_next_line = true;
                }
            } else if let Some((k, v)) = line.split_once('=') {
                if !v.trim().is_empty() {
                    result.push(format!("{}={}", k, mask_middle(v)));
                    continue;
                } else {
                    mask_next_line = true;
                }
            } else {
                // Space-separated key and value on the same line (e.g. "wifipass mysecret123")
                let parts: Vec<&str> = line.splitn(2, char::is_whitespace).collect();
                if parts.len() == 2 && !parts[1].trim().is_empty() {
                    result.push(format!("{} {}", parts[0], mask_middle(parts[1])));
                    continue;
                } else {
                    mask_next_line = true;
                }
            }
        }

        result.push(line.to_string());
    }

    let mut out = result.join("\n");
    if content.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Opens a native OS "Save As" dialog defaulting to Downloads and enforces a .txt extension
fn save_with_dialog(
    default_dir: Option<PathBuf>,
    default_stem: &str,
    content: &str,
) -> Result<String, String> {
    let clean_stem = default_stem
        .trim_end_matches(".txt")
        .trim_end_matches(".log");
    let filename = format!("{}.txt", clean_stem);

    let mut dialog = rfd::FileDialog::new()
        .set_file_name(&filename)
        .add_filter("Text Files (*.txt)", &["txt"]);

    if let Some(dir) = default_dir {
        let _ = std::fs::create_dir_all(&dir);
        dialog = dialog.set_directory(dir);
    }

    if let Some(save_path) = dialog.save_file() {
        let final_path = if save_path.extension().and_then(|e| e.to_str()) != Some("txt") {
            save_path.with_extension("txt")
        } else {
            save_path
        };

        std::fs::write(&final_path, content)
            .map_err(|e| format!("Failed to write file: {}", e))?;

        Ok(final_path.to_string_lossy().to_string())
    } else {
        Err("Save cancelled by user".to_string())
    }
}

#[tauri::command]
async fn save_to_downloads(
    app: tauri::AppHandle,
    filename: String,
    content: String,
) -> Result<String, String> {
    let download_dir = app.path().download_dir().ok();

    fn strip_ansi_codes(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    while let Some(&next_c) = chars.peek() {
                        chars.next();
                        if next_c.is_ascii_alphabetic() {
                            break;
                        }
                    }
                    continue;
                }
            }
            out.push(c);
        }
        out
    }

    let final_content = if filename.to_lowercase().contains("b2settings") {
        mask_b2settings_wifi_password(&content)
    } else {
        content
    };

    tokio::task::spawn_blocking(move || save_with_dialog(download_dir, &filename, &final_content))
        .await
        .map_err(|e| format!("Dialog task failed: {}", e))?
}

#[tauri::command]
fn connect_ssh(state: tauri::State<'_, SshState>, ip: String) -> Result<String, String> {
    use std::net::ToSocketAddrs;
    use std::time::Duration;

    let addr = format!("{}:22", ip)
        .to_socket_addrs()
        .map_err(|e| format!("Invalid IP address: {}", e))?
        .next()
        .ok_or("Could not resolve IP address")?;

    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(4))
        .map_err(|e| format!("TCP connection failed: {}", e))?;

    let mut sess = Session::new().map_err(|e| format!("Failed to create SSH session: {}", e))?;
    sess.set_tcp_stream(tcp);
    sess.handshake()
        .map_err(|e| format!("SSH handshake failed: {}", e))?;

    sess.userauth_password("root", "brennan")
        .map_err(|e| format!("Authentication failed: {}", e))?;

    let mut state_sess = state.session.lock().unwrap();
    let mut state_dir = state.current_dir.lock().unwrap();
    let mut state_ip = state.ip.lock().unwrap();

    *state_sess = Some(sess);
    *state_dir = String::from("/root");
    *state_ip = ip.clone();

    Ok(format!("[Success! Connected to root@{} via SSH]\n# ", ip))
}

#[tauri::command]
fn run_remote_command(
    state: tauri::State<'_, SshState>,
    command: String,
) -> Result<String, String> {
    let mut state_sess = state.session.lock().unwrap();
    let sess = state_sess.as_mut().ok_or("Not connected to SSH")?;

    let mut dir_lock = state.current_dir.lock().unwrap();
    let current_dir = dir_lock.clone();

    let wrapped_cmd = format!("cd {} && {}; printf '\\n'; pwd", current_dir, command);

    // Automatically add --color=always to ls commands for BusyBox colour output
    let colored_cmd = if command.trim() == "ls" || command.trim().starts_with("ls ") {
        format!("ls --color=always {}", &command.trim()[2..])
    } else {
        command.clone()
    };

    let wrapped_cmd = format!("cd {} && {}; printf '\\n'; pwd", current_dir, colored_cmd);

    let mut channel = sess
        .channel_session()
        .map_err(|e| format!("Failed to open channel: {}", e))?;

    channel
        .exec(&wrapped_cmd)
        .map_err(|e| format!("Failed to execute command: {}", e))?;

    let mut output = String::new();
    channel
        .read_to_string(&mut output)
        .map_err(|e| format!("Failed to read output: {}", e))?;

    let _ = channel.close();
    let _ = channel.wait_close();

    let mut lines: Vec<&str> = output.lines().collect();
    while let Some(last_line) = lines.last() {
        if last_line.trim().is_empty() {
            lines.pop();
        } else {
            break;
        }
    }

    if let Some(last_line) = lines.pop() {
        let trimmed_dir = last_line.trim();
        if trimmed_dir.starts_with('/') {
            *dir_lock = trimmed_dir.to_string();
        } else {
            lines.push(last_line);
        }
    }

    let clean_output = lines.join("\n");
    let final_output = if clean_output.trim().is_empty() {
        String::from("# ")
    } else {
        format!("{}\n# ", clean_output)
    };

    Ok(final_output)
}

#[tauri::command]
fn disconnect_ssh(state: tauri::State<'_, SshState>) -> Result<String, String> {
    let mut state_sess = state.session.lock().unwrap();
    let mut state_dir = state.current_dir.lock().unwrap();
    let mut state_ip = state.ip.lock().unwrap();

    *state_sess = None;
    *state_dir = String::from("/root");
    *state_ip = String::new();

    Ok(String::from("\n--- Disconnected ---\n"))
}

fn detect_local_subnet() -> Result<String, String> {
    use std::net::{IpAddr, UdpSocket};

    let routed_ip = UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .ok()
        .map(|addr| addr.ip());

    let resolved_ip = match routed_ip {
        Some(ip) if !ip.is_loopback() => Some(ip),
        _ => local_ip_address::local_ip().ok(),
    };

    match resolved_ip {
        Some(IpAddr::V4(ipv4)) => {
            let octets = ipv4.octets();
            if octets[0] == 169 && octets[1] == 254 {
                return Err("Network adapter has no DHCP address (169.254.x.x)".into());
            }
            Ok(format!("{}.{}.{}", octets[0], octets[1], octets[2]))
        }
        Some(IpAddr::V6(ipv6)) => Err(format!("Only IPv6 detected ({}), IPv4 required", ipv6)),
        None => Err("No active LAN interface detected".into()),
    }
}

#[tauri::command]
fn get_local_subnet() -> String {
    detect_local_subnet().unwrap_or_else(|_| String::from("192.168.1"))
}

#[tauri::command]
async fn scan_subnet(app: tauri::AppHandle) -> Result<String, String> {
    use std::net::SocketAddr;
    use std::time::Duration;
    use tauri::Emitter;

    let subnet = detect_local_subnet()?;

    let _ = app.emit(
        "scandisk-progress",
        format!(
            "[Scanning local network {}.1 through {}.254...]\n",
            subnet, subnet
        ),
    );

    for chunk_start in (1..=254).step_by(50) {
        let mut handles = Vec::new();

        for offset in 0..50 {
            let suffix = chunk_start + offset;
            if suffix > 254 {
                break;
            }
            let target_ip = format!("{}.{}", subnet, suffix);

            handles.push(tokio::task::spawn_blocking(move || -> Option<String> {
                let addr: SocketAddr = format!("{}:80", target_ip).parse().ok()?;
                let mut stream =
                    TcpStream::connect_timeout(&addr, Duration::from_millis(600)).ok()?;
                let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
                let _ = stream.set_write_timeout(Some(Duration::from_millis(1500)));

                let req = format!(
                    "GET /b2cgi.fcgi?getWebInfo HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                    target_ip
                );
                stream.write_all(req.as_bytes()).ok()?;

                let mut resp = String::new();
                stream.read_to_string(&mut resp).ok()?;

                if resp.contains("200 OK")
                    && resp
                        .find("\r\n\r\n")
                        .map(|p| resp.len() > p + 4)
                        .unwrap_or(false)
                {
                    Some(target_ip)
                } else {
                    None
                }
            }));
        }

        for h in handles {
            if let Ok(Some(found_ip)) = h.await {
                return Ok(found_ip);
            }
        }
    }

    Err("Error: No Brennan device found on subnet".into())
}

#[tauri::command]
async fn trigger_scandisk(
    state: tauri::State<'_, SshState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    use tauri::Emitter;

    let ip_lock = state.ip.lock().unwrap();
    let target_ip = ip_lock.clone();
    drop(ip_lock);

    if target_ip.is_empty() {
        return Err("Not connected to a device IP".into());
    }

    let _ = app.emit("scandisk-progress", "\n[Starting Scan Disk...]\n");

    let mut stream = TcpStream::connect(format!("{}:80", target_ip))
        .map_err(|e| format!("HTTP connection failed: {}", e))?;

    let request = format!(
        "GET /b2cgi.fcgi?scanDisk HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        target_ip
    );

    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("Failed to send HTTP request: {}", e))?;

    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut attempts = 0;
        let mut last_bg = String::new();

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            attempts += 1;

            if attempts >= 300 {
                break;
            }

            if let Ok(body) = tokio::task::spawn_blocking({
                let ip = target_ip.clone();
                move || {
                    let mut s = TcpStream::connect(format!("{}:80", ip)).ok()?;
                    let req = format!(
                        "GET /b2cgi.fcgi?status HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                        ip
                    );
                    s.write_all(req.as_bytes()).ok()?;
                    let mut resp = String::new();
                    s.read_to_string(&mut resp).ok()?;
                    let pos = resp.find("\r\n\r\n")?;
                    Some(resp[pos + 4..].to_string())
                }
            })
            .await
            {
                if let Some(data_str) = body {
                    if let Some(bg_idx) = data_str.find("\"background\"") {
                        let slice = &data_str[bg_idx..];
                        if let Some(colon_idx) = slice.find(':') {
                            let val_slice = &slice[colon_idx + 1..];
                            let cleaned = val_slice
                                .trim()
                                .trim_start_matches('"')
                                .trim_end_matches('"')
                                .trim_end_matches('}');
                            let bg_status = cleaned.split('"').next().unwrap_or("").trim();

                            if bg_status.is_empty() || bg_status.to_lowercase() == "idle" {
                                let _ = app_handle.emit(
                                    "scandisk-progress",
                                    "\n[Success! Scan Disk Complete!]\n# ",
                                );
                                break;
                            }

                            if bg_status != last_bg {
                                let digits: String = bg_status
                                    .chars()
                                    .skip_while(|c| !c.is_ascii_digit())
                                    .take_while(|c| c.is_ascii_digit())
                                    .collect();

                                let display_msg = if !digits.is_empty() {
                                    format!("    ---> Scanning Disk Files {}\n", digits)
                                } else {
                                    format!("    ---> {}\n", bg_status)
                                };

                                let _ = app_handle.emit("scandisk-progress", display_msg);
                                last_bg = bg_status.to_string();
                            }
                        }
                    }
                }
            }
        }
    });

    Ok(())
}

#[tauri::command]
async fn trigger_reboot(
    state: tauri::State<'_, SshState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    use std::time::Duration;
    use tauri::Emitter;

    let target_ip = {
        let ip_lock = state.ip.lock().unwrap();
        ip_lock.clone()
    };

    if target_ip.is_empty() {
        return Err("Not connected to a device IP".into());
    }

    {
        let mut state_sess = state.session.lock().unwrap();
        if let Some(sess) = state_sess.take() {
            sess.set_timeout(2000);
            if let Ok(mut channel) = sess.channel_session() {
                let _ = channel.exec(
                    "sh -c '(sleep 1; sync; echo s > /proc/sysrq-trigger; echo u > /proc/sysrq-trigger; echo b > /proc/sysrq-trigger) >/dev/null 2>&1 &'",
                );
                let _ = channel.close();
            }
        }
    }

    let _ = app.emit(
        "scandisk-progress",
        "\n--- Reboot command sent to Brennan ---\n",
    );

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = app_handle.emit(
            "scandisk-progress",
            "--- Waiting 12s for Brennan to shut down... ---\n",
        );
        tokio::time::sleep(Duration::from_secs(12)).await;

        let _ = app_handle.emit(
            "scandisk-progress",
            "--- Polling for device restart... ---\n",
        );

        let mut poll_attempts = 0;

        loop {
            poll_attempts += 1;
            let ip_clone = target_ip.clone();

            let reconnect_res = tokio::task::spawn_blocking(move || -> Option<Session> {
                use std::net::ToSocketAddrs;
                let addr = format!("{}:22", ip_clone)
                    .to_socket_addrs()
                    .ok()?
                    .next()?;
                let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).ok()?;
                let mut sess = Session::new().ok()?;
                sess.set_timeout(4000);
                sess.set_tcp_stream(tcp);
                sess.handshake().ok()?;
                sess.userauth_password("root", "brennan").ok()?;
                sess.set_timeout(0);
                Some(sess)
            })
            .await;

            if let Ok(Some(new_sess)) = reconnect_res {
                let mut verify_output = String::new();
                if let Ok(mut channel) = new_sess.channel_session() {
                    if channel.exec("echo '[SSH Verified: Brennan is back online]'").is_ok() {
                        let _ = channel.read_to_string(&mut verify_output);
                        let _ = channel.close();
                        let _ = channel.wait_close();
                    }
                }

                {
                    let ssh_state = app_handle.state::<SshState>();
                    let mut state_sess = ssh_state.session.lock().unwrap();
                    let mut state_dir = ssh_state.current_dir.lock().unwrap();
                    let mut state_ip = ssh_state.ip.lock().unwrap();

                    *state_sess = Some(new_sess);
                    *state_dir = String::from("/root");
                    *state_ip = target_ip.clone();
                }

                let _ = app_handle.emit(
                    "scandisk-progress",
                    "\n--- Auto-reconnect successful ---\n",
                );
                if !verify_output.is_empty() {
                    let _ = app_handle.emit("scandisk-progress", format!("{}# ", verify_output));
                } else {
                    let _ = app_handle.emit("scandisk-progress", "# ");
                }
                break;
            }

            if poll_attempts >= 30 {
                let _ = app_handle.emit(
                    "scandisk-progress",
                    "\n--- Auto-reconnect failed (timed out) ---\n",
                );
                break;
            } else {
                let _ = app_handle.emit(
                    "scandisk-progress",
                    format!("    ---> Waiting for SSH (Attempt {}/30)...\n", poll_attempts),
                );
            }

            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    unsafe {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    }

    tauri::Builder::default()
        .manage(SshState {
            session: Mutex::new(None),
            current_dir: Mutex::new(String::from("/root")),
            ip: Mutex::new(String::new()),
        })
        .invoke_handler(tauri::generate_handler![
            connect_ssh,
            run_remote_command,
            disconnect_ssh,
            get_local_subnet,
            scan_subnet,
            trigger_scandisk,
            trigger_reboot,
            save_to_downloads,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
