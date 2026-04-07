use std::io::{self, BufRead, Write};
use serde_json::{json, Value};

mod db;
mod sm2;
mod tools;

fn main() {
    let db = match db::Database::open() {
        Ok(db) => db,
        Err(e) => {
            eprintln!("lersi: failed to open database: {:#}", e);
            std::process::exit(1);
        }
    };

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Notifications have no "id" — don't respond.
        let id = match request.get("id") {
            Some(id) => id.clone(),
            None => continue,
        };

        let method = request["method"].as_str().unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(json!({}));

        let response = match dispatch(&db, method, &params) {
            Ok(result) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            }),
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32000,
                    "message": format!("{:#}", e)
                }
            }),
        };

        let _ = writeln!(stdout, "{}", serde_json::to_string(&response).unwrap());
        let _ = stdout.flush();
    }
}

fn dispatch(db: &db::Database, method: &str, params: &Value) -> anyhow::Result<Value> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "lersi", "version": env!("CARGO_PKG_VERSION") }
        })),

        "tools/list" => Ok(json!({ "tools": tools::list() })),

        "tools/call" => {
            let name = params["name"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("tools/call: missing 'name'"))?;
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            tools::call(db, name, &args)
        }

        "ping" => Ok(json!({})),

        _ => Err(anyhow::anyhow!("method not found: {}", method)),
    }
}
