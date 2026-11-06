//! OpenAI-compatible `/chat/completions` and `/embeddings` shapes (NFR-COMP-05), as in the M1
//! harness (`09-model-check.md` §2).

use serde_json::{Value, json};

use crate::error::{EgressError, ErrorCode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> ChatMessage {
        ChatMessage {
            role: "system".into(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> ChatMessage {
        ChatMessage {
            role: "user".into(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> ChatMessage {
        ChatMessage {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

/// A JSON schema the reply must follow (`response_format: json_schema`).
#[derive(Clone, Debug, PartialEq)]
pub struct ReplySchema {
    pub name: String,
    pub schema: Value,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub schema: Option<ReplySchema>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub seed: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatReply {
    pub content: String,
    /// The model the endpoint says answered.
    pub model: Option<String>,
    pub finish_reason: Option<String>,
    pub ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Embeddings {
    /// In input order.
    pub vectors: Vec<Vec<f32>>,
    pub model: Option<String>,
    pub ms: u64,
}

pub(crate) fn chat_body(model: &str, req: &ChatRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();
    let mut body = json!({ "model": model, "messages": messages, "stream": false });
    if let Some(s) = &req.schema {
        body["response_format"] = json!({
            "type": "json_schema",
            "json_schema": { "name": s.name, "strict": true, "schema": s.schema },
        });
    }
    if let Some(n) = req.max_tokens {
        body["max_tokens"] = json!(n);
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(s) = req.seed {
        body["seed"] = json!(s);
    }
    body
}

fn not_openai() -> EgressError {
    EgressError::new(ErrorCode::NotOpenAi, "No OpenAI-compatible API at this URL")
}

pub(crate) fn parse_chat(body: &str, ms: u64) -> Result<ChatReply, EgressError> {
    let v: Value = serde_json::from_str(body).map_err(|_| not_openai())?;
    let choice = v["choices"].get(0).ok_or_else(not_openai)?;
    let content = choice["message"]["content"]
        .as_str()
        .ok_or_else(|| EgressError::new(ErrorCode::BadReply, "The reply had no message text"))?
        .to_string();
    Ok(ChatReply {
        content,
        model: v["model"].as_str().map(str::to_string),
        finish_reason: choice["finish_reason"].as_str().map(str::to_string),
        ms,
    })
}

pub(crate) fn embeddings_body(model: &str, inputs: &[String]) -> Value {
    json!({ "model": model, "input": inputs })
}

pub(crate) fn parse_embeddings(
    body: &str,
    inputs: usize,
    ms: u64,
) -> Result<Embeddings, EgressError> {
    let v: Value = serde_json::from_str(body).map_err(|_| not_openai())?;
    let data = v["data"].as_array().ok_or_else(not_openai)?;
    let mut rows: Vec<(u64, Vec<f32>)> = data
        .iter()
        .map(|d| {
            let idx = d["index"].as_u64().unwrap_or(0);
            let vec = d["embedding"]
                .as_array()
                .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
                .unwrap_or_default();
            (idx, vec)
        })
        .collect();
    rows.sort_by_key(|r| r.0);
    if rows.len() != inputs || rows.iter().any(|r| r.1.is_empty()) {
        return Err(EgressError::new(
            ErrorCode::BadReply,
            format!("{} embeddings came back for {inputs} texts", rows.len()),
        ));
    }
    Ok(Embeddings {
        vectors: rows.into_iter().map(|r| r.1).collect(),
        model: v["model"].as_str().map(str::to_string),
        ms,
    })
}
