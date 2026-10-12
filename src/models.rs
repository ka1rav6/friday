//! Model provider abstraction. Nebius Token Factory (hosted NVIDIA Nemotron) is
//! the primary provider; Ollama and a deterministic mock are alternates.
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// A message body. Plain text for ordinary turns; `Parts` carries the
/// OpenAI-style content array used for image and audio input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<Value>),
}

impl Content {
    /// The text of a message, ignoring any attached media.
    pub fn as_text(&self) -> String {
        match self {
            Content::Text(t) => t.clone(),
            Content::Parts(p) => p
                .iter()
                .filter_map(|x| x.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Msg {
    pub role: String,
    pub content: Content,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Msg {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: Content::Text(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }
    pub fn tool_result(id: &str, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: Content::Text(content.into()),
            tool_calls: None,
            tool_call_id: Some(id.into()),
        }
    }
    /// A user turn carrying an image alongside the prompt, for vision models.
    pub fn with_image(prompt: &str, data_uri: &str) -> Self {
        Self {
            role: "user".into(),
            content: Content::Parts(vec![
                json!({"type": "text", "text": prompt}),
                json!({"type": "image_url", "image_url": {"url": data_uri}}),
            ]),
            tool_calls: None,
            tool_call_id: None,
        }
    }
    /// A user turn carrying audio, for models that accept it directly.
    pub fn with_audio(prompt: &str, base64_audio: &str, format: &str) -> Self {
        Self {
            role: "user".into(),
            content: Content::Parts(vec![
                json!({"type": "text", "text": prompt}),
                json!({"type": "input_audio",
                       "input_audio": {"data": base64_audio, "format": format}}),
            ]),
            tool_calls: None,
            tool_call_id: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub messages: Vec<Msg>,
    /// OpenAI-style tool schemas.
    pub tools: Vec<Value>,
    pub max_tokens: u32,
    pub temperature: f32,
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // `model`/`latency_ms` are recorded for observability
pub struct ModelResponse {
    pub content: String,
    pub tool_calls: Vec<ToolInvocation>,
    pub raw_assistant: Value,
    pub usage: Usage,
    pub model: String,
    pub latency_ms: u64,
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;
    fn model_id(&self) -> &str;
    /// True when the provider needs the network. Used by offline routing.
    fn requires_network(&self) -> bool;
    async fn generate(&self, request: ModelRequest) -> Result<ModelResponse>;
    async fn healthy(&self) -> bool;

    /// Transcribes audio through the provider's speech-to-text endpoint.
    /// The default refuses rather than pretending: a provider that has no such
    /// endpoint should not silently return an empty transcript.
    async fn transcribe(&self, _audio: Vec<u8>, _filename: &str, _model: &str) -> Result<String> {
        bail!("provider {} has no speech-to-text endpoint", self.name())
    }

    /// Whether `generate` may be given a message containing image parts.
    /// Reported, not assumed - a text-only model will error on an image.
    fn supports_images(&self) -> bool {
        false
    }

    /// The same provider pointed at a different model id, for per-role model
    /// tiering. `None` means this provider serves exactly one model, which is
    /// true of the mock and is why tests are unaffected by tiering.
    fn with_model(&self, _model: &str) -> Option<Arc<dyn ModelProvider>> {
        None
    }
}

// ---------------------------------------------------------------- OpenAI-compatible

/// Covers both Nebius Token Factory and Ollama, which expose the same
/// `/chat/completions` shape.
pub struct OpenAiCompatible {
    pub label: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub http: reqwest::Client,
    pub network: bool,
    /// Set when the configured model is known to accept image parts.
    pub vision: bool,
}

impl OpenAiCompatible {
    pub fn new(
        label: &str,
        base_url: &str,
        model: &str,
        api_key: Option<String>,
        timeout_secs: u64,
        network: bool,
    ) -> Result<Self> {
        Ok(Self {
            label: label.into(),
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.into(),
            api_key,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(timeout_secs))
                .build()
                .context("build http client")?,
            network,
            vision: false,
        })
    }

    /// Marks this provider as able to take image parts. Set from
    /// `media.vision_model`, so the user declares it rather than FRIDAY
    /// guessing from a model name.
    pub fn with_vision(mut self, vision: bool) -> Self {
        self.vision = vision;
        self
    }
}

impl OpenAiCompatible {
    /// Actionable message for a provider we could not reach at all.
    fn unreachable_hint(&self) -> String {
        if self.label == "ollama" {
            format!(
                "cannot reach the local model server at {}. Either start it (`ollama serve`, then \
                 `ollama pull {}`), or set NEBIUS_API_KEY in .env to use the hosted NVIDIA Nemotron \
                 model through Nebius Token Factory.",
                self.base_url.trim_end_matches("/v1"),
                self.model
            )
        } else {
            format!(
                "cannot reach {} at {}. Check the network connection and that the base URL and API \
                 key in .env are correct.",
                self.label, self.base_url
            )
        }
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatible {
    fn name(&self) -> &str {
        &self.label
    }
    fn model_id(&self) -> &str {
        &self.model
    }
    fn requires_network(&self) -> bool {
        self.network
    }

    async fn generate(&self, req: ModelRequest) -> Result<ModelResponse> {
        let mut body = json!({
            "model": self.model,
            "messages": req.messages,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
        });
        if !req.tools.is_empty() {
            body["tools"] = Value::Array(req.tools.clone());
            body["tool_choice"] = json!("auto");
        }
        let url = format!("{}/chat/completions", self.base_url);

        let mut last_err = None;
        for attempt in 0..3u32 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
            }
            let started = std::time::Instant::now();
            let mut rb = self.http.post(&url).json(&body);
            if let Some(k) = &self.api_key {
                rb = rb.bearer_auth(k);
            }
            let resp = match rb.send().await {
                Ok(r) => r,
                Err(e) if e.is_connect() || e.is_timeout() => {
                    // A transport failure is almost always a setup problem, so say
                    // what to do about it rather than echoing the socket error.
                    last_err = Some(anyhow::anyhow!("{}", self.unreachable_hint()));
                    continue;
                }
                Err(e) => {
                    last_err = Some(anyhow::anyhow!("{}: request failed: {e}", self.label));
                    continue;
                }
            };
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 429 || status.is_server_error() {
                last_err = Some(anyhow::anyhow!(
                    "{}: provider returned {status}: {}",
                    self.label,
                    crate::util::truncate(&crate::util::redact(&text), 400)
                ));
                continue;
            }
            if !status.is_success() {
                bail!(
                    "{}: provider returned {status}: {}",
                    self.label,
                    crate::util::truncate(&crate::util::redact(&text), 400)
                );
            }
            let v: Value = serde_json::from_str(&text).context("provider returned invalid JSON")?;
            return parse_completion(v, &self.model, started.elapsed().as_millis() as u64);
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("{}: exhausted retries", self.label)))
    }

    async fn healthy(&self) -> bool {
        let url = format!("{}/models", self.base_url);
        let mut rb = self.http.get(&url).timeout(Duration::from_secs(8));
        if let Some(k) = &self.api_key {
            rb = rb.bearer_auth(k);
        }
        matches!(rb.send().await, Ok(r) if r.status().is_success())
    }

    fn supports_images(&self) -> bool {
        self.vision
    }

    fn with_model(&self, model: &str) -> Option<Arc<dyn ModelProvider>> {
        if model == self.model {
            return None;
        }
        Some(Arc::new(Self {
            label: self.label.clone(),
            base_url: self.base_url.clone(),
            model: model.to_string(),
            api_key: self.api_key.clone(),
            // A reqwest::Client is an Arc internally, so cloning it reuses the
            // same connection pool rather than opening a second one.
            http: self.http.clone(),
            network: self.network,
            vision: self.vision,
        }))
    }

    async fn transcribe(&self, audio: Vec<u8>, filename: &str, model: &str) -> Result<String> {
        let part = reqwest::multipart::Part::bytes(audio)
            .file_name(filename.to_string())
            .mime_str(crate::media::mime_for(std::path::Path::new(filename)))
            .context("audio part")?;
        let form = reqwest::multipart::Form::new()
            .text("model", model.to_string())
            .text("response_format", "json")
            .part("file", part);

        let url = format!("{}/audio/transcriptions", self.base_url);
        let mut rb = self.http.post(&url).multipart(form);
        if let Some(k) = &self.api_key {
            rb = rb.bearer_auth(k);
        }
        let resp = rb.send().await.map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                anyhow::anyhow!("{}", self.unreachable_hint())
            } else {
                anyhow::anyhow!("{}: transcription request failed: {e}", self.label)
            }
        })?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::NOT_FOUND {
            bail!(
                "{} has no /audio/transcriptions endpoint, or model {model:?} is not available \
                 there. Set media.stt_model to a local whisper.cpp model for offline \
                 transcription instead.",
                self.label
            );
        }
        if !status.is_success() {
            bail!(
                "{}: transcription returned {status}: {}",
                self.label,
                crate::util::truncate(&crate::util::redact(&text), 400)
            );
        }
        let v: Value =
            serde_json::from_str(&text).context("transcription response was not JSON")?;
        v.get("text")
            .and_then(|t| t.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("transcription response had no `text` field"))
    }
}

fn parse_completion(v: Value, model: &str, latency_ms: u64) -> Result<ModelResponse> {
    let choice = v
        .pointer("/choices/0/message")
        .ok_or_else(|| anyhow::anyhow!("provider response missing choices[0].message"))?
        .clone();
    let content = choice
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let mut tool_calls = Vec::new();
    if let Some(arr) = choice.get("tool_calls").and_then(|t| t.as_array()) {
        for (i, tc) in arr.iter().enumerate() {
            let name = tc
                .pointer("/function/name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let raw = tc
                .pointer("/function/arguments")
                .cloned()
                .unwrap_or(json!("{}"));
            let arguments = match raw {
                Value::String(s) => serde_json::from_str(&s).unwrap_or(json!({})),
                other => other,
            };
            let id = tc
                .get("id")
                .and_then(|x| x.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| format!("call_{i}"));
            tool_calls.push(ToolInvocation {
                id,
                name,
                arguments,
            });
        }
    }
    Ok(ModelResponse {
        content,
        tool_calls,
        raw_assistant: choice,
        usage: Usage {
            prompt_tokens: v
                .pointer("/usage/prompt_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0) as u32,
            completion_tokens: v
                .pointer("/usage/completion_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0) as u32,
        },
        model: v
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or(model)
            .to_string(),
        latency_ms,
    })
}

// ---------------------------------------------------------------- Mock

/// Deterministic provider for tests and for the no-credential dev path.
/// It never pretends to be the hosted NVIDIA model: `name()` is "mock".
pub struct MockProvider {
    pub scripted: std::sync::Mutex<Vec<ModelResponse>>,
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)] // scripted responses are used by the agent-loop tests
impl MockProvider {
    pub fn new() -> Self {
        Self {
            scripted: std::sync::Mutex::new(Vec::new()),
        }
    }
    pub fn push(&self, r: ModelResponse) {
        self.scripted.lock().unwrap().insert(0, r);
    }
    pub fn text(content: &str) -> ModelResponse {
        ModelResponse {
            content: content.into(),
            tool_calls: vec![],
            raw_assistant: json!({"role":"assistant","content":content}),
            usage: Usage::default(),
            model: "mock".into(),
            latency_ms: 0,
        }
    }
    pub fn call(name: &str, args: Value) -> ModelResponse {
        ModelResponse {
            content: String::new(),
            tool_calls: vec![ToolInvocation {
                id: "call_0".into(),
                name: name.into(),
                arguments: args.clone(),
            }],
            raw_assistant: json!({"role":"assistant","content":null,"tool_calls":[
                {"id":"call_0","type":"function","function":{"name":name,"arguments":args.to_string()}}]}),
            usage: Usage::default(),
            model: "mock".into(),
            latency_ms: 0,
        }
    }
}

#[async_trait]
impl ModelProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        "mock"
    }
    fn requires_network(&self) -> bool {
        false
    }
    async fn generate(&self, _req: ModelRequest) -> Result<ModelResponse> {
        let next = self.scripted.lock().unwrap().pop();
        Ok(next.unwrap_or_else(|| MockProvider::text("mock: no scripted response remaining")))
    }
    async fn healthy(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------- Router

pub struct Router {
    pub hosted: Option<Arc<dyn ModelProvider>>,
    pub local: Option<Arc<dyn ModelProvider>>,
    /// Separate provider for the vision model, which is rarely the same model
    /// as the main reasoning one. `None` means no vision model is configured.
    pub vision: Option<Arc<dyn ModelProvider>>,
    pub mock: Arc<dyn ModelProvider>,
}

impl Default for Router {
    /// A router with no real provider, so tests and the no-credential path get
    /// the mock rather than a surprise network call.
    fn default() -> Self {
        Self {
            hosted: None,
            local: None,
            vision: None,
            mock: Arc::new(MockProvider::new()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Route {
    pub provider: String,
    pub model: String,
    pub reason: String,
}

impl Router {
    /// Picks a provider for `mode` (auto|online|offline). Returns an error
    /// rather than silently downgrading a privacy-sensitive request.
    pub fn select(&self, mode: &str) -> Result<(Arc<dyn ModelProvider>, Route)> {
        let pick = |p: &Arc<dyn ModelProvider>, reason: &str| {
            (
                p.clone(),
                Route {
                    provider: p.name().into(),
                    model: p.model_id().into(),
                    reason: reason.into(),
                },
            )
        };
        match mode {
            "offline" => {
                if let Some(l) = &self.local {
                    Ok(pick(l, "offline mode: using local model"))
                } else {
                    bail!(
                        "offline mode requires a local model; none is configured or reachable. \
                           Start Ollama and set model.ollama_model, or switch mode to auto/online."
                    )
                }
            }
            "online" => {
                if let Some(h) = &self.hosted {
                    Ok(pick(
                        h,
                        "online mode: using hosted NVIDIA model via Nebius Token Factory",
                    ))
                } else {
                    bail!("online mode requires NEBIUS_API_KEY; it is not set")
                }
            }
            _ => {
                if let Some(h) = &self.hosted {
                    Ok(pick(h, "auto: hosted NVIDIA model available"))
                } else if let Some(l) = &self.local {
                    Ok(pick(
                        l,
                        "auto: no hosted credential, falling back to local model",
                    ))
                } else {
                    Ok(pick(
                        &self.mock,
                        "auto: no model provider configured; mock responses only",
                    ))
                }
            }
        }
    }
}

pub fn build_router(cfg: &crate::config::Config) -> Router {
    let key = std::env::var("NEBIUS_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty());
    let hosted = key.and_then(|k| {
        OpenAiCompatible::new(
            "nebius",
            &cfg.model.nebius_base_url,
            &cfg.model.nebius_model,
            Some(k),
            cfg.model.request_timeout_secs,
            true,
        )
        .ok()
        .map(|p| Arc::new(p) as Arc<dyn ModelProvider>)
    });
    let local = OpenAiCompatible::new(
        "ollama",
        &format!("{}/v1", cfg.model.ollama_base_url.trim_end_matches('/')),
        &cfg.model.ollama_model,
        None,
        cfg.model.request_timeout_secs,
        false,
    )
    .ok()
    .map(|p| Arc::new(p) as Arc<dyn ModelProvider>);
    // The vision provider needs a credential, so it exists only alongside a
    // hosted one, and only when the user actually named a vision model.
    let vision = if cfg.media.vision_model.is_empty() {
        None
    } else {
        std::env::var("NEBIUS_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .and_then(|k| {
                OpenAiCompatible::new(
                    "nebius",
                    &cfg.model.nebius_base_url,
                    &cfg.media.vision_model,
                    Some(k),
                    cfg.model.request_timeout_secs,
                    true,
                )
                .ok()
                .map(|p| Arc::new(p.with_vision(true)) as Arc<dyn ModelProvider>)
            })
    };
    Router {
        hosted,
        local,
        vision,
        mock: Arc::new(MockProvider::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_calls_with_string_arguments() {
        let v = json!({"model":"m","choices":[{"message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"c1","type":"function","function":{"name":"get_current_time","arguments":"{\"tz\":\"UTC\"}"}}]}}],
            "usage":{"prompt_tokens":5,"completion_tokens":7}});
        let r = parse_completion(v, "m", 1).unwrap();
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "get_current_time");
        assert_eq!(r.tool_calls[0].arguments["tz"], "UTC");
        assert_eq!(r.usage.completion_tokens, 7);
    }

    #[test]
    fn offline_without_local_model_fails_honestly() {
        let r = Router::default();
        let e = match r.select("offline") {
            Ok(_) => panic!("expected offline to fail"),
            Err(e) => e.to_string(),
        };
        assert!(e.contains("local model"));
        assert!(r.select("online").is_err());
        assert_eq!(r.select("auto").unwrap().1.provider, "mock");
    }
}
