// AI integration via Ollama: send a prompt to a local Ollama server's
// /api/chat endpoint and return the assistant's reply.
//
// This backs the Skill BASIC statement `PROMPT <expr>`, which evaluates the
// expression as a string, sends it to the LLM, and prints the reply:
//
//   10 PROMPT "write a haiku about apples"
//
// Ollama can use local or cloud-hosted models. Prompts sent to a cloud model or
// remote host leave the machine. Configuration is resolved with this precedence:
//   1. Environment variables OLLAMA_HOST / OLLAMA_MODEL (highest).
//   2. The JSON config file (see `AiConfig::from_file`).
//   3. Built-in defaults (localhost:11434, llama3.2).
//
// All network access is blocking: the interpreter is a synchronous tree-walker
// so a `PROMPT` statement simply stalls execution until Ollama responds,
// exactly like `INPUT` does while waiting on stdin. A 120-second timeout keeps
// a slow model from hanging the program forever.

use std::env;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::time::Duration;

const DEFAULT_HOST: &str = "http://localhost:11434";
const DEFAULT_MODEL: &str = "llama3.2";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

/// The built-in default system prompt, used when the settings file doesn't
/// supply one. Kept minimal and specific to this interpreter's context so
/// out-of-the-box behavior is predictable; override globally via the `system`
/// field in `.env/settings.json`.
const DEFAULT_SYSTEM: &str = "You are a helpful assistant responding to a \
Skill BASIC program. Keep replies concise and direct.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Ollama,
    PiRpc,
}

#[derive(Debug, Clone, Default)]
pub struct PiConfig {
    pub model: Option<String>,
    pub system: Option<String>,
    pub tools: bool,
    pub timeout_seconds: Option<u64>,
}

/// Resolved AI configuration: where to reach Ollama and which model to ask for.
///
/// `main` builds one of these at startup (env vars + settings file + defaults)
/// and threads it through `Interp` so every `PROMPT` call uses the same
/// settings without re-reading the environment or the file each time.
#[derive(Debug, Clone)]
pub struct AiConfig {
    pub engine: Engine,
    pub host: String,
    pub model: String,
    pub system: String,
    pub pi: PiConfig,
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            engine: Engine::Ollama,
            host: DEFAULT_HOST.to_string(),
            model: DEFAULT_MODEL.to_string(),
            system: DEFAULT_SYSTEM.to_string(),
            pi: PiConfig::default(),
        }
    }
}

/// On-disk shape of the JSON config file. All fields are optional; anything
/// absent falls through to the built-in default (and then env vars override
/// host/model on top via `with_env_overlays`).
#[derive(Debug, Default, serde::Deserialize)]
struct ConfigFile {
    host: Option<String>,
    model: Option<String>,
    system: Option<String>,
    pi: Option<PiConfigFile>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct PiConfigFile {
    model: Option<String>,
    system: Option<String>,
    tools: Option<bool>,
    #[serde(rename = "timeoutSeconds")]
    timeout_seconds: Option<u64>,
}

impl AiConfig {
    /// Read a JSON config file and merge its fields over the built-in
    /// defaults. A missing file is not an error — it just means "use the
    /// defaults". A malformed file *is* an error so the user notices.
    ///
    /// The expected file shape is:
    ///   { "host": "http://localhost:11434", "model": "llama3.2",
    ///     "system": "optional system prompt" }
    /// All keys are optional.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let mut cfg = AiConfig::default();
        match std::fs::metadata(path) {
            Ok(meta) if meta.len() > MAX_CONFIG_BYTES => {
                return Err(format!(
                    "?AI ERROR: {} exceeds the {} byte configuration limit",
                    path.display(),
                    MAX_CONFIG_BYTES
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(cfg),
            Err(e) => {
                return Err(format!(
                    "?AI ERROR: cannot inspect {}: {}",
                    path.display(),
                    e
                ));
            }
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => return Err(format!("?AI ERROR: cannot read {}: {}", path.display(), e)),
        };
        let file: ConfigFile = serde_json::from_str(&text)
            .map_err(|e| format!("?AI ERROR: cannot parse {}: {}", path.display(), e))?;
        if let Some(h) = file.host {
            cfg.host = h;
        }
        if let Some(m) = file.model {
            cfg.model = m;
        }
        if let Some(s) = file.system {
            cfg.system = s;
        }
        if let Some(pi) = file.pi {
            cfg.pi.model = pi.model;
            cfg.pi.system = pi.system;
            cfg.pi.tools = pi.tools.unwrap_or(false);
            cfg.pi.timeout_seconds = pi.timeout_seconds;
        }
        Ok(cfg)
    }

    /// Apply environment-variable overrides over an already-loaded config.
    /// `OLLAMA_HOST` / `OLLAMA_MODEL` win over the file and defaults if set; if
    /// absent, the existing value is kept.
    pub fn with_env_overlays(mut self) -> Self {
        if let Ok(h) = env::var("OLLAMA_HOST") {
            self.host = h;
        }
        if let Ok(m) = env::var("OLLAMA_MODEL") {
            self.model = m;
        }
        self
    }
}

/// A single message in the running conversation, in the OpenAI/Ollama chat
/// schema. We keep the history on `Interp` and send the whole list with each
/// `PROMPT` call so the model sees prior turns and can answer follow-up
/// questions like "how many tests passed?" with full context.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatMessage {
    pub role: String, // "user" or "assistant"
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage {
            role: "user".to_string(),
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        ChatMessage {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
}

/// Send the running conversation plus a new user message to Ollama's
/// `/api/chat` and return the assistant's text reply.
///
/// `system` is an optional system prompt prepended to the message list (it
/// steers model persona/format for the whole conversation). When `None`, no
/// system message is sent — the caller decides whether to use a default.
/// `history` is the prior turns (user/assistant alternating, or empty for a
/// fresh conversation); `user_prompt` is the new user message to append. The
/// caller is responsible for recording the new user turn and the returned
/// assistant turn into the history so subsequent calls stay continuous.
///
/// Errors are returned in the same `?... ERROR` style as the rest of the
/// interpreter (e.g. `?SYNTAX ERROR`, `?ILLEGAL QUANTITY ERROR`) so the caller
/// surfaces them the same way it surfaces other runtime errors. The most
/// common failure is Ollama not running locally — that produces a
/// connection-refused error which we surface as a clear `?AI ERROR` rather
/// than a panic, so a missing Ollama server degrades gracefully inside a
/// BASIC program.
pub fn complete(
    cfg: &AiConfig,
    system: Option<&str>,
    history: &[ChatMessage],
    user_prompt: &str,
) -> Result<String, String> {
    let url = reqwest::Url::parse(&format!("{}/api/chat", cfg.host.trim_end_matches('/')))
        .map_err(|e| format!("?AI ERROR: invalid Ollama host: {}", e))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(
            "?AI ERROR: Ollama host must be an HTTP(S) URL without embedded credentials".into(),
        );
    }

    // Build the full message list: optional system prompt, prior turns, then
    // the new user turn. Ollama's /api/chat accepts the same `messages` array
    // as the OpenAI schema, with the system message first.
    let mut messages: Vec<serde_json::Value> = Vec::new();
    if let Some(sys) = system {
        messages.push(serde_json::json!({ "role": "system", "content": sys }));
    }
    messages.extend(history.iter().map(|m| serde_json::to_value(m).unwrap()));
    messages.push(serde_json::json!({ "role": "user", "content": user_prompt }));

    // `stream: false` makes Ollama return one JSON object with the full reply
    // instead of an SSE stream of chunks, which is simpler to consume from a
    // blocking call.
    let body = serde_json::json!({
        "model": cfg.model,
        "stream": false,
        "messages": messages
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("?AI ERROR: could not build HTTP client: {}", e))?;

    let mut resp = client
        .post(url)
        .json(&body)
        .send()
        .map_err(|e| format!("?AI ERROR: request failed (is Ollama running?): {}", e))?;

    let status = resp.status();
    if resp
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE_BYTES)
    {
        return Err(format!(
            "?AI ERROR: response exceeds the {} byte limit",
            MAX_RESPONSE_BYTES
        ));
    }
    let mut bytes = Vec::new();
    resp.by_ref()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("?AI ERROR: could not read response body: {}", e))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(format!(
            "?AI ERROR: response exceeds the {} byte limit",
            MAX_RESPONSE_BYTES
        ));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| "?AI ERROR: response body is not valid UTF-8".to_string())?;

    if !status.is_success() {
        // Trim Ollama's JSON error body so the error line stays readable;
        // keep the status code and a short snippet for debugging.
        let snippet: String = text.chars().take(160).collect();
        return Err(format!("?AI ERROR: HTTP {}: {}", status, snippet));
    }

    let parsed: ChatResponse = serde_json::from_str(&text)
        .map_err(|e| format!("?AI ERROR: could not parse response: {}", e))?;

    parsed.message.content.trim().to_string().ok_if_nonempty()
}

/// Streaming variant of `complete` for callers that want to print model output
/// as it arrives while still receiving the complete reply for chat history.
pub fn complete_stream<F>(
    cfg: &AiConfig,
    system: Option<&str>,
    history: &[ChatMessage],
    user_prompt: &str,
    mut on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    let url = reqwest::Url::parse(&format!("{}/api/chat", cfg.host.trim_end_matches('/')))
        .map_err(|e| format!("?AI ERROR: invalid Ollama host: {}", e))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(
            "?AI ERROR: Ollama host must be an HTTP(S) URL without embedded credentials".into(),
        );
    }

    let mut messages: Vec<serde_json::Value> = Vec::new();
    if let Some(sys) = system {
        messages.push(serde_json::json!({ "role": "system", "content": sys }));
    }
    messages.extend(history.iter().map(|m| serde_json::to_value(m).unwrap()));
    messages.push(serde_json::json!({ "role": "user", "content": user_prompt }));

    let body = serde_json::json!({
        "model": cfg.model,
        "stream": true,
        "messages": messages
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("?AI ERROR: could not build HTTP client: {}", e))?;

    let mut resp = client
        .post(url)
        .json(&body)
        .send()
        .map_err(|e| format!("?AI ERROR: request failed (is Ollama running?): {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        let mut bytes = Vec::new();
        resp.by_ref()
            .take(MAX_RESPONSE_BYTES.min(4096))
            .read_to_end(&mut bytes)
            .map_err(|e| format!("?AI ERROR: could not read response body: {}", e))?;
        let text = String::from_utf8_lossy(&bytes);
        let snippet: String = text.chars().take(160).collect();
        return Err(format!("?AI ERROR: HTTP {}: {}", status, snippet));
    }

    let mut reader = BufReader::new(resp);
    let mut line = String::new();
    let mut total_bytes: u64 = 0;
    let mut reply = String::new();

    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|e| format!("?AI ERROR: could not read response stream: {}", e))?;
        if read == 0 {
            break;
        }
        total_bytes += read as u64;
        if total_bytes > MAX_RESPONSE_BYTES {
            return Err(format!(
                "?AI ERROR: response exceeds the {} byte limit",
                MAX_RESPONSE_BYTES
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let parsed: ChatStreamResponse = serde_json::from_str(trimmed)
            .map_err(|e| format!("?AI ERROR: could not parse streamed response: {}", e))?;
        if let Some(error) = parsed.error {
            return Err(format!("?AI ERROR: {}", error));
        }
        if let Some(message) = parsed.message
            && !message.content.is_empty()
        {
            on_delta(&message.content)?;
            reply.push_str(&message.content);
        }
        if parsed.done.unwrap_or(false) {
            break;
        }
    }

    reply.trim().to_string().ok_if_nonempty()
}

// ---- minimal serde model for Ollama's /api/chat response ----
//
// We only deserialize the fields we read; `serde_json` ignores the rest.
// Ollama returns a top-level `message` object with `role` and `content`,
// plus metadata like `total_duration`, `eval_count`, etc. that we drop here.

#[derive(serde::Deserialize)]
struct ChatResponse {
    message: Message,
}

#[derive(serde::Deserialize)]
struct Message {
    content: String,
}

#[derive(serde::Deserialize)]
struct ChatStreamResponse {
    message: Option<Message>,
    done: Option<bool>,
    error: Option<String>,
}

// Small helper: turn an empty reply into an error so callers never get a
// silent blank string back from the LLM.
trait OkIfNonempty {
    fn ok_if_nonempty(self) -> Result<String, String>;
}
impl OkIfNonempty for String {
    fn ok_if_nonempty(self) -> Result<String, String> {
        if self.is_empty() {
            Err("?AI ERROR: empty response from model".to_string())
        } else {
            Ok(self)
        }
    }
}

// ---- tests ----
// The network path is exercised manually against a running Ollama; here we
// just check the pure pieces (response parsing, env defaults).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_ollama_response() {
        // Shape matches Ollama's /api/chat (non-streaming) output, trimmed to
        // the fields we actually deserialize.
        let raw = r#"{
            "model": "llama3.2",
            "message": { "role": "assistant", "content": "hello there" },
            "done": true,
            "total_duration": 123456789
        }"#;
        let parsed: ChatResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.message.content, "hello there");
    }

    #[test]
    fn trims_whitespace_in_reply() {
        let raw = r#"{ "message": { "role": "assistant", "content": "\n  hi  \n" } }"#;
        let parsed: ChatResponse = serde_json::from_str(raw).unwrap();
        let reply = parsed.message.content.trim().to_string();
        assert_eq!(reply, "hi");
    }

    #[test]
    fn empty_reply_is_an_error() {
        let s = String::new();
        assert!(s.ok_if_nonempty().is_err());
    }

    // ---- config file loading ----

    #[test]
    fn missing_config_file_yields_defaults() {
        // A path that does not exist is not an error; we fall back to the
        // built-in defaults so the interpreter works with no config file.
        let cfg = AiConfig::from_file(Path::new("/no/such/settings.json")).unwrap();
        assert_eq!(cfg.host, DEFAULT_HOST);
        assert_eq!(cfg.model, DEFAULT_MODEL);
    }

    #[test]
    fn config_file_overrides_defaults() {
        let tmp = std::env::temp_dir().join("applesoft_ai_settings_test.json");
        std::fs::write(
            &tmp,
            r#"{ "host": "http://192.168.1.10:11434", "model": "gemma3" }"#,
        )
        .unwrap();
        let cfg = AiConfig::from_file(&tmp).unwrap();
        assert_eq!(cfg.host, "http://192.168.1.10:11434");
        assert_eq!(cfg.model, "gemma3");
        // No `system` key => built-in default survives.
        assert_eq!(cfg.system, DEFAULT_SYSTEM);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn config_file_sets_system_prompt() {
        let tmp = std::env::temp_dir().join("applesoft_ai_settings_system.json");
        std::fs::write(
            &tmp,
            r#"{ "system": "You are a pirate. Reply in pirate speak." }"#,
        )
        .unwrap();
        let cfg = AiConfig::from_file(&tmp).unwrap();
        assert_eq!(cfg.system, "You are a pirate. Reply in pirate speak.");
        // Other fields fall through to defaults.
        assert_eq!(cfg.host, DEFAULT_HOST);
        assert_eq!(cfg.model, DEFAULT_MODEL);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn config_file_sets_pi_options() {
        let tmp = std::env::temp_dir().join("skillbasic_ai_settings_pi.json");
        std::fs::write(
            &tmp,
            r#"{
                "pi": {
                    "model": "provider/model",
                    "system": "Be concise",
                    "tools": true,
                    "timeoutSeconds": 300
                }
            }"#,
        )
        .unwrap();
        let cfg = AiConfig::from_file(&tmp).unwrap();
        assert_eq!(cfg.pi.model.as_deref(), Some("provider/model"));
        assert_eq!(cfg.pi.system.as_deref(), Some("Be concise"));
        assert!(cfg.pi.tools);
        assert_eq!(cfg.pi.timeout_seconds, Some(300));
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn config_file_partial_uses_defaults_for_missing_fields() {
        // Only `model` specified; `host` should fall through to the default.
        let tmp = std::env::temp_dir().join("applesoft_ai_settings_partial.json");
        std::fs::write(&tmp, r#"{ "model": "qwen3" }"#).unwrap();
        let cfg = AiConfig::from_file(&tmp).unwrap();
        assert_eq!(cfg.host, DEFAULT_HOST);
        assert_eq!(cfg.model, "qwen3");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn malformed_config_file_is_an_error() {
        let tmp = std::env::temp_dir().join("applesoft_ai_settings_bad.json");
        std::fs::write(&tmp, r#"{ this is not json"#).unwrap();
        let err = AiConfig::from_file(&tmp).unwrap_err();
        assert!(err.starts_with("?AI ERROR"), "got: {}", err);
        assert!(err.contains("cannot parse"), "got: {}", err);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn oversized_config_file_is_rejected() {
        let tmp = std::env::temp_dir().join("applesoft_ai_settings_oversized.json");
        std::fs::write(&tmp, vec![b' '; MAX_CONFIG_BYTES as usize + 1]).unwrap();
        let err = AiConfig::from_file(&tmp).unwrap_err();
        assert!(err.contains("configuration limit"), "got: {}", err);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn env_overrides_win_over_config_file() {
        let tmp = std::env::temp_dir().join("applesoft_ai_settings_env.json");
        std::fs::write(
            &tmp,
            r#"{ "host": "http://from-file:11434", "model": "from-file-model" }"#,
        )
        .unwrap();
        let cfg = AiConfig::from_file(&tmp).unwrap().with_env_overlays();
        // No env vars set during tests => file values should survive.
        assert_eq!(cfg.host, "http://from-file:11434");
        assert_eq!(cfg.model, "from-file-model");
        let _ = std::fs::remove_file(&tmp);
    }
}
