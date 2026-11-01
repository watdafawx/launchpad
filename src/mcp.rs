//! `launchpad --mcp`: a Model Context Protocol server over stdio.
//!
//! Read tools show the config, startup items and last-run log. The only write
//! is `suggest_changes`, which files a suggestion for the user to review in
//! Settings. The server runs without admin rights and can't touch the config
//! itself (the config folder is admin-only), so a model can never make
//! launchpad run something as admin without the user approving it.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

use crate::config;
use crate::startup;
use crate::suggest::{self, Op, Suggestion};

const INSTRUCTIONS: &str = "launchpad replaces the Windows startup list on this PC. It starts apps at login \
in a set order: each app can wait for another app ('after'), wait a number of seconds ('delay'), and wait \
until CPU use is low ('cpu_below'). Apps run as the normal user unless 'admin' is true. Managed apps \
have their Windows startup entry kept switched off.\n\
You can read the setup, but you can't change it directly: call suggest_changes, and the user reviews the \
suggestion in launchpad's Settings window (tray icon > Settings…) and clicks Apply, then Save. Tell the \
user to do that after suggesting. Group related changes into one suggestion with a clear summary and \
reason. Only suggest admin=true for apps that need elevation (fan, tuning or system tools). Good \
defaults: password manager, VPN/mesh and audio tools at login; chat apps 20–60 s later; launchers, music \
and sync tools 1–3 min later or with cpu_below 25.";

pub fn run() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut client = String::from("an MCP client");

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                send(&mut out, json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":e.to_string()}}));
                continue;
            }
        };
        // Notifications (no id) need no reply.
        let Some(id) = msg.get("id").cloned() else { continue };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        let reply = match msg.get("method").and_then(Value::as_str).unwrap_or("") {
            "initialize" => {
                if let Some(n) = params.pointer("/clientInfo/name").and_then(Value::as_str) {
                    client = n.to_string();
                }
                let version = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18");
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "launchpad", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => Ok(call(&params, &client)),
            _ => Err("Method not found"),
        };
        send(
            &mut out,
            match reply {
                Ok(r) => json!({"jsonrpc":"2.0","id":id,"result":r}),
                Err(m) => json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":m}}),
            },
        );
    }
}

fn send(out: &mut impl Write, v: Value) {
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn text(s: String, is_error: bool) -> Value {
    json!({"content":[{"type":"text","text":s}],"isError":is_error})
}

fn call(params: &Value, client: &str) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    match run_tool(name, &args, client) {
        Ok(s) => text(s, false),
        Err(e) => text(e, true),
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn run_tool(name: &str, args: &Value, client: &str) -> Result<String, String> {
    match name {
        "get_setup" => {
            let cfg = config::load()?;
            let apps: Vec<Value> = cfg
                .apps
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let mut v = serde_json::to_value(a).unwrap_or_default();
                    v["position"] = json!(i);
                    v["estimated_start_s"] = json!(suggest::estimate(&cfg, i));
                    v
                })
                .collect();
            Ok(pretty(&json!({
                "installed": config::config_path().exists(),
                "stay_resident": cfg.stay_resident,
                "guard": cfg.guard,
                "apps": apps,
                "pending_suggestions": suggest::load().len(),
            })))
        }
        "list_startup_items" => {
            let cfg = config::load().unwrap_or_default();
            let items: Vec<Value> = startup::scan()
                .into_iter()
                .map(|it| {
                    let managed = cfg.apps.iter().find(|a| a.startup.as_ref() == Some(&it.r)).map(|a| a.name.clone());
                    json!({
                        "name": it.r.name,
                        "source": it.r.source,
                        "command": it.command,
                        "enabled_in_windows": it.enabled,
                        "managed_by": managed,
                    })
                })
                .collect();
            Ok(pretty(&json!(items)))
        }
        "get_last_run_log" => Ok(std::fs::read_to_string(config::log_path())
            .unwrap_or_else(|_| "No run logged yet.".into())),
        "list_suggestions" => Ok(pretty(&serde_json::to_value(suggest::load()).unwrap_or_default())),
        "suggest_changes" => {
            let summary = args.get("summary").and_then(Value::as_str).unwrap_or("").trim().to_string();
            if summary.is_empty() {
                return Err("summary is required".into());
            }
            let reason = args.get("reason").and_then(Value::as_str).unwrap_or("").to_string();
            let ops: Vec<Op> = serde_json::from_value(args.get("ops").cloned().unwrap_or(json!([])))
                .map_err(|e| format!("ops: {e}"))?;
            if ops.is_empty() {
                return Err("ops is empty".into());
            }
            // Check it applies cleanly to the current config.
            let cfg = config::load()?;
            let mut test = cfg.clone();
            suggest::apply(&mut test, &ops)?;

            let s = Suggestion {
                id: suggest::new_id(),
                created: suggest::now(),
                source: client.to_string(),
                summary,
                reason,
                ops,
            };
            let preview: Vec<String> = s.ops.iter().flat_map(|o| suggest::describe(&cfg, o)).map(|(t, _)| t).collect();
            let mut list = suggest::load();
            list.push(s.clone());
            suggest::save(&list)?;
            Ok(format!(
                "Suggestion {} saved. The user will see:\n{}\n\nAsk them to open launchpad Settings \
                 (right-click the tray icon > Settings…), click Apply on the suggestion, then Save changes.",
                s.id,
                preview.join("\n")
            ))
        }
        "withdraw_suggestion" => {
            let id = args.get("id").and_then(Value::as_str).ok_or("id is required")?;
            if suggest::remove(id)? {
                Ok(format!("Withdrew {id}."))
            } else {
                Err(format!("No suggestion with id {id}."))
            }
        }
        _ => Err(format!("Unknown tool: {name}")),
    }
}

fn tools() -> Value {
    let app_fields = json!({
        "name": {"type":"string"},
        "command": {"type":"string","description":"Full command line, a .lnk path, or shell:AppsFolder\\<AUMID>. %VARS% are expanded. Quote paths with spaces."},
        "enabled": {"type":"boolean"},
        "delay": {"type":"number","description":"Seconds to wait after 'after' (or after login). 0–900."},
        "after": {"type":"string","description":"Name of another app to wait for. Empty string = login."},
        "cpu_below": {"type":"integer","description":"Also wait until total CPU stays below this percent (5–95). 0 = off."},
        "admin": {"type":"boolean","description":"Run elevated. Only for apps that need it."},
        "process": {"type":"string","description":"Exe name used to skip the app if already running. Empty = auto."}
    });
    let mut add_props = app_fields.clone();
    add_props["startup"] = json!({
        "type":"object",
        "description":"Windows startup entry this app replaces (from list_startup_items). launchpad will keep it switched off.",
        "properties":{"source":{"type":"string","enum":["hkcu_run","hklm_run","hklm_run32","user_folder","common_folder"]},"name":{"type":"string"}},
        "required":["source","name"]
    });

    json!([
        {
            "name": "get_setup",
            "description": "Current launchpad setup: options and every app in start order, with rules and estimated start time (seconds after login, CPU waits not included).",
            "inputSchema": {"type":"object","properties":{}},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "list_startup_items",
            "description": "Every Windows startup entry (Run keys and Startup folders): its command, whether Windows will start it, and which launchpad app manages it, if any.",
            "inputSchema": {"type":"object","properties":{}},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "get_last_run_log",
            "description": "Log of the last startup run: when each app started (seconds since launchpad started), skips and failures.",
            "inputSchema": {"type":"object","properties":{}},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "list_suggestions",
            "description": "Suggestions waiting for the user to review.",
            "inputSchema": {"type":"object","properties":{}},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "suggest_changes",
            "description": "File a suggested change for the user to review and apply in launchpad Settings. Nothing changes until they approve. Ops apply in order; later ops can refer to apps added by earlier ones. App names must match get_setup exactly.",
            "inputSchema": {
                "type":"object",
                "properties":{
                    "summary":{"type":"string","description":"One line, e.g. 'Stagger chat apps after login'."},
                    "reason":{"type":"string","description":"Why, in a sentence or two."},
                    "ops":{"type":"array","items":{"oneOf":[
                        {"type":"object","properties":{"op":{"const":"add_app"},"app":{"type":"object","properties":add_props,"required":["name","command"]}},"required":["op","app"]},
                        {"type":"object","properties":{"op":{"const":"update_app"},"name":{"type":"string"},"changes":{"type":"object","properties":app_fields}},"required":["op","name","changes"]},
                        {"type":"object","properties":{"op":{"const":"remove_app"},"name":{"type":"string"}},"required":["op","name"]},
                        {"type":"object","properties":{"op":{"const":"move_app"},"name":{"type":"string"},"position":{"type":"integer","description":"0-based"}},"required":["op","name","position"]},
                        {"type":"object","properties":{"op":{"const":"set_options"},"stay_resident":{"type":"boolean"},"guard":{"type":"boolean"}},"required":["op"]}
                    ]}}
                },
                "required":["summary","ops"]
            }
        },
        {
            "name": "withdraw_suggestion",
            "description": "Remove a pending suggestion you filed.",
            "inputSchema": {"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}
        }
    ])
}
