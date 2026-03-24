// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{Read, Write};
use std::net::TcpStream;

use braid_lib::hook_server::HookRequest;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(about = "Braid – Claude Code GUI")]
struct Cli {
    /// Run as a hook handler. Value is the hook event name (e.g. "pre_tool_use").
    #[arg(long = "hook")]
    hook: Option<String>,

    /// The conversation node ID this hook invocation is associated with.
    #[arg(long = "node")]
    node: Option<String>,

    /// The hook server port to connect to.
    #[arg(long = "hook-port")]
    hook_port: Option<u16>,
}

fn main() {
    let cli = Cli::parse();

    if let Some(ref event) = cli.hook {
        handle_hook(event, cli.node.as_deref(), cli.hook_port);
        return;
    }

    // Normal Tauri app startup.
    braid_lib::run()
}

/// Called when the binary is invoked as a hook by Claude Code.
///
/// Reads the hook input from stdin, forwards it to the main Tauri process
/// via TCP, waits for a decision, and prints it to stdout.
fn log_hook(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("braid-hook.log"))
    {
        let _ = writeln!(f, "[{}] {msg}", chrono::Local::now().format("%H:%M:%S%.3f"));
    }
}

fn handle_hook(event: &str, node: Option<&str>, port: Option<u16>) {
    log_hook(&format!("hook invoked: event={event} node={node:?} port={port:?}"));

    let deny = |reason: &str| {
        log_hook(&format!("denying: {reason}"));
        let output = braid_lib::hooks::HookOutput::deny(reason);
        println!("{}", serde_json::to_string(&output).unwrap());
    };

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    let port = match port {
        Some(p) => p,
        None => {
            eprintln!("[hook] no --hook-port specified");
            deny("hook server unavailable");
            return;
        }
    };

    let input_value: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[hook] failed to parse stdin: {e}");
            deny("hook failed to parse input");
            return;
        }
    };

    let request = HookRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        node_id: node.map(String::from),
        event: event.to_string(),
        input: input_value,
    };

    let request_json = match serde_json::to_string(&request) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("[hook] failed to serialize request: {e}");
            deny("hook internal error");
            return;
        }
    };

    let mut stream = match TcpStream::connect(format!("127.0.0.1:{port}")) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[hook] failed to connect to hook server on port {port}: {e}");
            deny("hook server unreachable");
            return;
        }
    };

    if let Err(e) = writeln!(stream, "{request_json}") {
        eprintln!("[hook] failed to send request: {e}");
        deny("hook communication error");
        return;
    }
    stream.flush().ok();

    let mut response = String::new();
    if let Err(e) = stream.read_to_string(&mut response) {
        eprintln!("[hook] failed to read response: {e}");
        deny("hook response error");
        return;
    }

    let trimmed = response.trim();
    if trimmed.is_empty() {
        deny("empty response from hook server");
    } else {
        println!("{trimmed}");
    }
}
