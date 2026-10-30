use ssh2::Session;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex;

pub struct SshState {
    pub session: Mutex<Option<Session>>,
    pub current_dir: Mutex<String>,
    pub ip: Mutex<String>,
}

#[tauri::command]
fn connect_ssh(state: tauri::State<'_, SshState>, ip: String) -> Result<String, String> {
    let tcp = TcpStream::connect(format!("{}:22", ip))
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

    Ok(format!("[Success! Connected to root@{} via SSH]\n", ip))
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

    let wrapped_cmd = format!("cd {} && {}; pwd", current_dir, command);

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
    if let Some(last_line) = lines.pop() {
        if last_line.starts_with('/') {
            *dir_lock = last_line.trim().to_string();
        }
    }

    let clean_output = lines.join("\n");
    let final_output = if clean_output.is_empty() {
        output
    } else {
        format!("{}\n", clean_output)
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
        format!("[Scanning local network {}.1 through {}.254...]\n", subnet, subnet),
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
                let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(600)).ok()?;
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
                    && resp.find("\r\n\r\n").map(|p| resp.len() > p + 4).unwrap_or(false)
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
                                    "\n[Success! Scan Disk Complete!]\n",
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
                                    format!("   ---> Scanning Disk Files {}\n", digits)
                                } else {
                                    format!("   ---> {}\n", bg_status)
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
        if let Some(sess) = state_sess.as_mut() {
            if let Ok(mut channel) = sess.channel_session() {
                let _ = channel.exec(
                    "sh -c 'sync && echo s > /proc/sysrq-trigger && echo u > /proc/sysrq-trigger && echo b > /proc/sysrq-trigger'",
                );
            }
        }
    }

    let _ = app.emit(
        "scandisk-progress",
        "\n--- Reboot command sent to Brennan ---\n",
    );

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        use tauri::Manager;

        tokio::time::sleep(Duration::from_secs(4)).await;
        let _ = app_handle.emit("scandisk-progress", "--- Waiting for reboot... ---\n");

        {
            let ssh_state = app_handle.state::<SshState>();
            let mut state_sess = ssh_state.session.lock().unwrap();
            *state_sess = None;
        }

        tokio::time::sleep(Duration::from_secs(10)).await;
        let _ = app_handle.emit(
            "scandisk-progress",
            "--- Polling for device restart... ---\n",
        );

        let mut poll_attempts = 0;

        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
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
                sess.set_tcp_stream(tcp);
                sess.handshake().ok()?;
                sess.userauth_password("root", "brennan").ok()?;
                Some(sess)
            })
            .await;

            if let Ok(Some(new_sess)) = reconnect_res {
                let mut verify_output = String::new();
                if let Ok(mut channel) = new_sess.channel_session() {
                    if channel.exec("echo 'reboot successful'").is_ok() {
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
                    let _ = app_handle.emit("scandisk-progress", verify_output);
                }
                break;
            }

            if poll_attempts >= 30 {
                let _ = app_handle.emit("scandisk-progress", "\n--- Auto-reconnect failed ---\n");
                break;
            } else {
                let _ = app_handle.emit(
                    "scandisk-progress",
                    format!("   [Poll attempt {}/30...]\n", poll_attempts),
                );
            }
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
