//! OpenAI-compatible chat and embeddings over loopback (`ureq`, no TLS).

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

/// An agent that returns 4xx/5xx as responses, not errors, so status codes can be checked.
pub fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .build()
        .into()
}

/// GET → status code, or `None` when nothing answers.
pub fn status(agent: &ureq::Agent, url: &str, key: Option<&str>) -> Option<u16> {
    let mut req = agent.get(url);
    if let Some(k) = key {
        req = req.header("Authorization", &format!("Bearer {k}"));
    }
    req.call().ok().map(|r| r.status().as_u16())
}

pub fn post_status(agent: &ureq::Agent, url: &str, key: Option<&str>, body: &Value) -> Option<u16> {
    let mut req = agent.post(url);
    if let Some(k) = key {
        req = req.header("Authorization", &format!("Bearer {k}"));
    }
    req.send_json(body).ok().map(|r| r.status().as_u16())
}

pub struct Client {
    pub base: String,
    pub key: String,
    agent: ureq::Agent,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct Chat {
    pub content: String,
    pub finish_reason: Option<String>,
    pub usage: Usage,
    pub ms: u64,
}

impl Client {
    pub fn new(base: String, key: String) -> Self {
        Self {
            base,
            key,
            agent: agent(Duration::from_secs(600)),
        }
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let mut resp = self
            .agent
            .post(&format!("{}{path}", self.base))
            .header("Authorization", &format!("Bearer {}", self.key))
            .send_json(body)
            .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .with_config()
            .limit(256 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| e.to_string())?;
        if status != 200 {
            return Err(format!(
                "HTTP {status}: {}",
                text.chars().take(500).collect::<String>()
            ));
        }
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    /// One chat completion. `body` is completed with nothing; the caller builds it.
    pub fn chat(&self, body: &Value) -> Result<Chat, String> {
        let t = Instant::now();
        let v = self.post("/v1/chat/completions", body)?;
        let choice = &v["choices"][0];
        Ok(Chat {
            content: choice["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            finish_reason: choice["finish_reason"].as_str().map(str::to_string),
            usage: serde_json::from_value(v["usage"].clone()).unwrap_or_default(),
            ms: t.elapsed().as_millis() as u64,
        })
    }

    /// Embeddings in input order.
    pub fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let v = self.post("/v1/embeddings", &json!({ "input": inputs }))?;
        let mut rows: Vec<(u64, Vec<f32>)> = v["data"]
            .as_array()
            .ok_or("no data in embeddings response")?
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
        if rows.len() != inputs.len() {
            return Err(format!(
                "{} embeddings for {} inputs",
                rows.len(),
                inputs.len()
            ));
        }
        Ok(rows.into_iter().map(|r| r.1).collect())
    }
}
