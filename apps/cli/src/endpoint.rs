//! `mangkon-cli endpoint test` (M5a, US-09.6 AC2/AC3, NFR-COMP-05): tests an OpenAI-compatible
//! endpoint with the app's fixed test texts, without saving anything. No project is involved
//! (INV-EG-08). The key comes from an environment variable, so it is never in the shell
//! history or the process list, and it is never printed.

use std::process::ExitCode;

use mk_core::{NO_STRUCTURED_OUTPUT, SecretString, TestReport};
use serde_json::{Value, json};

pub struct TestOptions {
    pub base_url: String,
    pub model: String,
    /// The environment variable holding the API key.
    pub api_key_env: Option<String>,
    pub json: bool,
}

fn report_json(r: &TestReport) -> Value {
    json!({
        "ok": r.ok(),
        "origin": r.origin.key(),
        "addresses": r.addresses.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "class": r.class.map(|c| c.as_str()),
        "chat": match &r.chat {
            Ok(c) => json!({"ok": true, "reported_model": c.reported_model, "response_ms": c.response_ms}),
            Err(e) => json!({"ok": false, "code": e.code.as_str(), "message": e.message}),
        },
        "structured_output": r.structured_output,
        "embeddings": match &r.embeddings {
            None => Value::Null,
            Some(Ok(e)) => json!({"ok": true, "dimensions": e.dimensions, "response_ms": e.response_ms}),
            Some(Err(e)) => json!({"ok": false, "code": e.code.as_str(), "message": e.message}),
        },
        "summary": r.summary(),
    })
}

fn print_text(r: &TestReport) {
    let addresses: Vec<String> = r.addresses.iter().map(ToString::to_string).collect();
    println!("Endpoint           {}", r.origin.key());
    println!(
        "Addresses          {}",
        if addresses.is_empty() {
            "-".to_string()
        } else {
            addresses.join(", ")
        }
    );
    println!("Destination        {}", r.class.map_or("-", |c| c.label()));
    match &r.chat {
        Ok(c) => println!(
            "Chat               ok: {} in {} ms",
            c.reported_model.as_deref().unwrap_or("(no model name)"),
            c.response_ms
        ),
        Err(e) => println!(
            "Chat               failed ({}): {}",
            e.code.as_str(),
            e.message
        ),
    }
    let structured = match r.structured_output {
        None => "not tested",
        Some(true) => "supported",
        Some(false) => NO_STRUCTURED_OUTPUT,
    };
    println!("Structured output  {structured}");
    match &r.embeddings {
        None => println!("Embeddings         not tested"),
        Some(Ok(e)) => println!(
            "Embeddings         ok: {} dimensions in {} ms",
            e.dimensions, e.response_ms
        ),
        Some(Err(e)) => println!(
            "Embeddings         failed ({}): {}",
            e.code.as_str(),
            e.message
        ),
    }
}

/// Exits 0 when the chat call worked, 1 when it did not.
pub fn test(opts: &TestOptions) -> ExitCode {
    let key = match &opts.api_key_env {
        None => None,
        Some(var) => match std::env::var(var) {
            Ok(k) if !k.is_empty() => Some(SecretString::from(k)),
            _ => {
                eprintln!("error: the environment variable {var} is not set");
                return ExitCode::from(2);
            }
        },
    };
    let report = match mk_core::test_endpoint_at(&opts.base_url, &opts.model, key) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    if opts.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report_json(&report)).expect("JSON values")
        );
    } else {
        print_text(&report);
    }
    if report.ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
