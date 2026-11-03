//! `mangkon-cli endpoint test` (M5a, US-09.6 AC2/AC3, NFR-COMP-05, NFR-SEC-07) against an
//! in-process fake OpenAI-compatible server.

use std::net::{IpAddr, Ipv4Addr};
use std::process::{Command, Output};

use mk_egress::testkit::{Behavior, FakeServer};
use serde_json::Value;

const KEY: &str = "mk-test-key-cli-Jh3Wq8LmZ2pR5tYv";
const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

fn cli(args: &[&str], key: Option<&str>) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_mangkon-cli"));
    c.args(args).env_remove("MK_TEST_KEY");
    if let Some(k) = key {
        c.env("MK_TEST_KEY", k);
    }
    c.output().unwrap()
}

fn no_key_in(out: &Output) {
    for stream in [&out.stdout, &out.stderr] {
        assert!(!String::from_utf8_lossy(stream).contains(KEY));
    }
}

#[test]
fn endpoint_test_reports_and_never_prints_the_key() {
    let server = FakeServer::start(
        LOOPBACK,
        Behavior::OpenAi {
            model: "qwen-test".into(),
            structured: false,
            embeddings: true,
            key: Some(KEY.into()),
        },
    );
    let url = format!("http://127.0.0.1:{}", server.port());
    let args = [
        "endpoint",
        "test",
        &url,
        "--model",
        "qwen-test",
        "--api-key-env",
        "MK_TEST_KEY",
    ];

    let out = cli(&[&args[..], &["--json"]].concat(), Some(KEY));
    no_key_in(&out);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["origin"], format!("http://127.0.0.1:{}", server.port()));
    assert_eq!(v["class"], "this_machine");
    assert_eq!(v["chat"]["reported_model"], "qwen-test");
    assert_eq!(v["structured_output"], false);
    assert_eq!(v["embeddings"]["dimensions"], 3);

    let out = cli(&args, Some(KEY));
    no_key_in(&out);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("Chat               ok: qwen-test in "),
        "{text}"
    );
    assert!(
        text.contains("Structured output not supported — results may be less reliable"),
        "{text}"
    );

    // A wrong key: plain words, exit 1.
    let out = cli(&[&args[..], &["--json"]].concat(), Some("wrong-key"));
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["chat"]["code"], "key_refused");
    assert_eq!(v["chat"]["message"], "The endpoint refused the API key");

    // A missing variable or a bad URL is a usage error.
    let out = cli(&args, None);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("MK_TEST_KEY is not set"));
    let out = cli(&["endpoint", "test", "ftp://x", "--model", "m"], None);
    assert_eq!(out.status.code(), Some(2));
}
