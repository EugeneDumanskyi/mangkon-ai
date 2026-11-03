//! The endpoint test (US-09.6 AC2/AC3, NFR-COMP-05). It takes no project at all: it sends only
//! the fixed texts below (INV-EG-08), so it is not project egress and is not recorded as such.
//! It goes through the same pinned transport as the gate.

use std::net::IpAddr;
use std::time::Duration;

use secrecy::SecretString;
use serde_json::{Value, json};

use crate::error::EgressError;
use crate::openai::{
    ChatMessage, ChatRequest, ReplySchema, chat_body, embeddings_body, parse_chat, parse_embeddings,
};
use crate::policy::{BaseUrl, DestinationClass, Origin};
use crate::resolve::{Resolve, resolve_origin};
use crate::transport::{TlsRoots, pinned_agent, post_json, status_error};

/// The only text a chat test sends (INV-EG-08).
pub const TEST_TEXT: &str = "This is a connection test from Mangkon AI. Say hello in one word.";
/// The only text an embeddings test sends (INV-EG-08).
pub const TEST_EMBED_TEXT: &str = "Mangkon AI connection test";
/// What US-09.6 AC3 says when structured output is missing.
pub const NO_STRUCTURED_OUTPUT: &str =
    "Structured output not supported — results may be less reliable";
/// How long one test call may take: a local runtime may load the model on the first call.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Token limit for the structured-output probe, with room for a thinking model's reasoning.
const STRUCTURED_PROBE_MAX_TOKENS: u32 = 1024;

/// The structured-output probe's schema. The prompt never mentions JSON, so a reply that
/// follows it shows the endpoint applied the schema.
fn probe_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "greeting": { "type": "string" },
            "words": { "type": "integer" }
        },
        "required": ["greeting", "words"],
        "additionalProperties": false
    })
}

/// Whether `content` is exactly an object of the probe schema.
fn follows_probe_schema(content: &str) -> bool {
    let Ok(Value::Object(m)) = serde_json::from_str::<Value>(content.trim()) else {
        return false;
    };
    m.len() == 2
        && m.get("greeting").is_some_and(Value::is_string)
        && m.get("words").is_some_and(|w| w.is_i64() || w.is_u64())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatProbe {
    /// The model name in the reply.
    pub reported_model: Option<String>,
    pub response_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddingsProbe {
    pub dimensions: usize,
    pub response_ms: u64,
}

/// What a test found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestReport {
    pub origin: Origin,
    /// The addresses the host resolved to, all of them.
    pub addresses: Vec<IpAddr>,
    pub class: Option<DestinationClass>,
    /// The chat call: the one the test passes or fails on.
    pub chat: Result<ChatProbe, EgressError>,
    /// `None` when the chat call failed and the probe was not made.
    pub structured_output: Option<bool>,
    /// Reported, not required: an endpoint has no role until M6.
    pub embeddings: Option<Result<EmbeddingsProbe, EgressError>>,
}

impl TestReport {
    pub fn ok(&self) -> bool {
        self.chat.is_ok()
    }

    /// One line for the UI and `endpoint_test.message`.
    pub fn summary(&self) -> String {
        match &self.chat {
            Ok(c) => {
                let mut s = format!(
                    "Connected: {} answered in {} ms",
                    c.reported_model.as_deref().unwrap_or("the model"),
                    c.response_ms
                );
                if self.structured_output == Some(false) {
                    s.push_str(". ");
                    s.push_str(NO_STRUCTURED_OUTPUT);
                }
                s
            }
            Err(e) => e.message.clone(),
        }
    }
}

/// What to test. The key is the one typed or stored, never logged.
pub struct TestTarget<'a> {
    pub base_url: &'a BaseUrl,
    pub model: &'a str,
    pub key: Option<&'a SecretString>,
}

/// Tests an endpoint with fixed texts: chat, then structured output, then embeddings.
pub fn endpoint_test(
    resolver: &dyn Resolve,
    roots: &TlsRoots,
    target: TestTarget<'_>,
) -> TestReport {
    let origin = target.base_url.origin().clone();
    let resolved = match resolve_origin(resolver, &origin) {
        Ok(r) => r,
        Err(e) => {
            return TestReport {
                origin,
                addresses: Vec::new(),
                class: None,
                chat: Err(e),
                structured_output: None,
                embeddings: None,
            };
        }
    };
    let agent = pinned_agent(&origin, &resolved.ips, roots, TEST_TIMEOUT);
    let post = |path: &str, body: &Value| -> Result<(String, u64), EgressError> {
        let r = post_json(
            &agent,
            &origin,
            &target.base_url.join(path),
            target.key,
            body,
        )?;
        if (200..300).contains(&r.status) {
            Ok((r.body, r.ms))
        } else {
            Err(status_error(r.status, &r.body, target.model, target.key))
        }
    };

    let plain = ChatRequest {
        messages: vec![ChatMessage::user(TEST_TEXT)],
        max_tokens: Some(16),
        temperature: Some(0.0),
        ..ChatRequest::default()
    };
    let chat = post("chat/completions", &chat_body(target.model, &plain))
        .and_then(|(body, ms)| parse_chat(&body, ms))
        .map(|r| ChatProbe {
            reported_model: r.model,
            response_ms: r.ms,
        });
    let mut report = TestReport {
        origin: origin.clone(),
        addresses: resolved.ips.clone(),
        class: Some(resolved.class),
        chat,
        structured_output: None,
        embeddings: None,
    };
    if report.chat.is_err() {
        return report;
    }

    let structured = ChatRequest {
        messages: vec![ChatMessage::user(TEST_TEXT)],
        schema: Some(ReplySchema {
            name: "connection_test".into(),
            schema: probe_schema(),
        }),
        // A thinking model spends tokens on reasoning before the reply; with a small limit
        // the content comes back empty and the probe would wrongly say "not supported"
        // (Qwen3.5-4B on llama-server used 222).
        max_tokens: Some(STRUCTURED_PROBE_MAX_TOKENS),
        temperature: Some(0.0),
    };
    report.structured_output = Some(
        post("chat/completions", &chat_body(target.model, &structured))
            .and_then(|(body, ms)| parse_chat(&body, ms))
            .is_ok_and(|r| follows_probe_schema(&r.content)),
    );

    let inputs = [TEST_EMBED_TEXT.to_string()];
    report.embeddings = Some(
        post("embeddings", &embeddings_body(target.model, &inputs))
            .and_then(|(body, ms)| parse_embeddings(&body, 1, ms))
            .map(|e| EmbeddingsProbe {
                dimensions: e.vectors[0].len(),
                response_ms: e.ms,
            }),
    );
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_schema_check() {
        assert!(follows_probe_schema(r#"{"greeting":"Hello","words":1}"#));
        assert!(follows_probe_schema(
            " {\"words\": 1, \"greeting\": \"Hi\"}\n"
        ));
        assert!(!follows_probe_schema("Hello!"));
        assert!(!follows_probe_schema(r#"{"greeting":"Hello"}"#));
        assert!(!follows_probe_schema(
            r#"{"greeting":"Hello","words":"one"}"#
        ));
        assert!(!follows_probe_schema(
            r#"{"greeting":"Hello","words":1,"x":2}"#
        ));
    }
}
