use std::fmt;
use std::time::Duration;

use serde::Deserialize;

/// Model role: small runs Map filtering, large runs Reduce convergence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Small,
    Large,
}

/// Model endpoint specification. Keys are resolved from env/files and never logged.
pub struct ModelSpec {
    pub role: Role,
    pub endpoint: String,
    pub model: String,
    pub key: String,
    pub timeout: Duration,
    pub max_retries: u32,
}

/// Custom Debug redacts secrets so keys cannot leak into logs or reports.
impl fmt::Debug for ModelSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelSpec")
            .field("role", &self.role)
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("key", &"<redacted>")
            .field("timeout", &self.timeout)
            .field("max_retries", &self.max_retries)
            .finish()
    }
}

/// Transport failures are classified for retry decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    Timeout,
    Status(u16),
    Network,
    BadBody,
}

impl TransportError {
    /// Retry only transient failures; bad bodies and status codes are terminal.
    fn transient(&self) -> bool {
        matches!(self, TransportError::Timeout | TransportError::Network)
    }
}

/// Network abstraction for deterministic timeout/body/breaker tests.
pub trait Transport: Send + Sync {
    fn post(
        &self,
        endpoint: &str,
        key: &str,
        body: &str,
        timeout: Duration,
    ) -> Result<String, TransportError>;
}

/// Consecutive-failure breaker. Once tripped, callers stop I/O instead of spinning.
#[derive(Debug)]
struct Breaker {
    consecutive: u32,
    threshold: u32,
}

impl Breaker {
    fn new(threshold: u32) -> Self {
        Self {
            consecutive: 0,
            threshold: threshold.max(1),
        }
    }
    fn tripped(&self) -> bool {
        self.consecutive >= self.threshold
    }
    fn ok(&mut self) {
        self.consecutive = 0;
    }
    fn fail(&mut self) {
        self.consecutive = self.consecutive.saturating_add(1);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CallError {
    Tripped,
    Exhausted,
}

/// Single model client with hard timeout, bounded retry, backoff, and breaker.
pub struct ModelClient {
    spec: ModelSpec,
    transport: Box<dyn Transport>,
    breaker: Breaker,
    backoff_base: Duration,
}

impl ModelClient {
    pub fn new(spec: ModelSpec, transport: Box<dyn Transport>, breaker_threshold: u32) -> Self {
        Self {
            spec,
            transport,
            breaker: Breaker::new(breaker_threshold),
            backoff_base: Duration::from_millis(50),
        }
    }

    pub fn role(&self) -> Role {
        self.spec.role
    }

    /// Send one completion request with bounded retry. Tripped breakers fail fast.
    pub fn complete(&mut self, prompt: &str) -> Result<String, CallError> {
        if self.breaker.tripped() {
            return Err(CallError::Tripped);
        }
        let body = build_chat_body(&self.spec.model, prompt);
        let mut attempt = 0u32;
        loop {
            match self.transport.post(
                &self.spec.endpoint,
                &self.spec.key,
                &body,
                self.spec.timeout,
            ) {
                Ok(raw) => match parse_content(&raw) {
                    Some(c) => {
                        self.breaker.ok();
                        return Ok(c);
                    }
                    None => self.breaker.fail(),
                },
                Err(e) => {
                    self.breaker.fail();
                    if !e.transient() {
                        return Err(CallError::Exhausted);
                    }
                }
            }
            if attempt >= self.spec.max_retries || self.breaker.tripped() {
                return Err(if self.breaker.tripped() {
                    CallError::Tripped
                } else {
                    CallError::Exhausted
                });
            }
            backoff(self.backoff_base, attempt);
            attempt += 1;
        }
    }
}

/// Registry routes small Map calls and the single large Reduce call by role.
#[derive(Default)]
pub struct Registry {
    pub small: Vec<ModelClient>,
    pub large: Option<ModelClient>,
}

impl Registry {
    pub fn has_large(&self) -> bool {
        self.large.is_some()
    }
    /// Missing large model means callers must degrade to AST-only output.
    pub fn degraded(&self) -> bool {
        self.large.is_none()
    }

    /// Run bounded Map waves over AST seed chunks. Failed chunks are dropped.
    pub fn map_small_pool(&mut self, seed: &str, max_parallel: usize) -> Option<String> {
        if self.small.is_empty() || seed.trim().is_empty() {
            return None;
        }

        let chunks = seed_chunks(seed, 16 * 1024);
        let wave_size = max_parallel.max(1).min(self.small.len());
        let mut out = Vec::new();
        let mut offset = 0usize;
        while offset < chunks.len() {
            let end = (offset + wave_size).min(chunks.len());
            let wave = &chunks[offset..end];
            let results = std::thread::scope(|scope| {
                let mut handles = Vec::new();
                for (client, chunk) in self.small.iter_mut().zip(wave.iter()) {
                    handles.push(scope.spawn(move || client.complete(&small_prompt(chunk))));
                }
                let mut results = Vec::new();
                for handle in handles {
                    if let Ok(Ok(text)) = handle.join() {
                        results.push(text);
                    }
                }
                results
            });
            out.extend(results);
            offset = end;
        }

        if out.is_empty() {
            None
        } else {
            Some(out.join("\n---\n"))
        }
    }
}

fn small_prompt(chunk: &str) -> String {
    format!(
        "You are sift's cheap Map-stage auditor. Read this dehydrated AST JSONL chunk and return only concise risk findings with file/line evidence. If no signal, return [].\nAST JSONL:\n{chunk}"
    )
}

fn seed_chunks(seed: &str, max_bytes: usize) -> Vec<String> {
    let max_bytes = max_bytes.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in seed.lines() {
        if !current.is_empty() && current.len() + line.len() + 1 > max_bytes {
            chunks.push(current);
            current = String::new();
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn backoff(base: Duration, attempt: u32) {
    let factor = 1u32.checked_shl(attempt).unwrap_or(u32::MAX);
    let wait = base.saturating_mul(factor).min(Duration::from_secs(2));
    std::thread::sleep(wait);
}

fn build_chat_body(model: &str, prompt: &str) -> String {
    serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
    })
    .to_string()
}

/// Parse OpenAI-style responses; missing fields or bad JSON count as failures.
fn parse_content(raw: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let c = v.get("choices")?.get(0)?.get("message")?.get("content")?;
    c.as_str().map(str::to_string)
}

/// Blocking ureq transport with a mandatory timeout.
pub struct UreqTransport;

impl Transport for UreqTransport {
    fn post(
        &self,
        endpoint: &str,
        key: &str,
        body: &str,
        timeout: Duration,
    ) -> Result<String, TransportError> {
        let resp = ureq::post(endpoint)
            .timeout(timeout)
            .set("authorization", &format!("Bearer {key}"))
            .set("content-type", "application/json")
            .send_string(body);
        match resp {
            Ok(r) => r.into_string().map_err(|_| TransportError::BadBody),
            Err(ureq::Error::Status(code, _)) => Err(TransportError::Status(code)),
            Err(ureq::Error::Transport(t)) => Err(match t.kind() {
                ureq::ErrorKind::Io => TransportError::Timeout,
                _ => TransportError::Network,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake {
        seq: Mutex<Vec<Result<String, TransportError>>>,
    }
    impl Fake {
        fn new(seq: Vec<Result<String, TransportError>>) -> Self {
            Self {
                seq: Mutex::new(seq),
            }
        }
    }
    impl Transport for Fake {
        fn post(&self, _: &str, _: &str, _: &str, _: Duration) -> Result<String, TransportError> {
            let Ok(mut seq) = self.seq.lock() else {
                return Err(TransportError::Network);
            };
            seq.pop().unwrap_or(Err(TransportError::Network))
        }
    }

    fn spec() -> ModelSpec {
        ModelSpec {
            role: Role::Small,
            endpoint: "x".into(),
            model: "m".into(),
            key: "secret".into(),
            timeout: Duration::from_millis(1),
            max_retries: 1,
        }
    }
    fn ok_body() -> String {
        "{\"choices\":[{\"message\":{\"content\":\"hi\"}}]}".into()
    }

    #[test]
    fn key_redacted_in_debug() {
        let dbg = format!("{:?}", spec());
        assert!(!dbg.contains("secret"));
        assert!(dbg.contains("redacted"));
    }

    #[test]
    fn good_response_parses_and_resets() {
        let mut c = ModelClient::new(spec(), Box::new(Fake::new(vec![Ok(ok_body())])), 3);
        assert_eq!(c.complete("p").unwrap_or_default(), "hi");
    }

    #[test]
    fn timeouts_trip_breaker() {
        let seq = vec![Err(TransportError::Timeout); 6];
        let mut c = ModelClient::new(spec(), Box::new(Fake::new(seq)), 2);
        assert_eq!(c.complete("p"), Err(CallError::Tripped));
    }

    #[test]
    fn bad_status_not_retried_exhausts() {
        let mut c = ModelClient::new(
            spec(),
            Box::new(Fake::new(vec![Err(TransportError::Status(401))])),
            3,
        );
        assert_eq!(c.complete("p"), Err(CallError::Exhausted));
    }

    #[test]
    fn bad_json_counts_as_failure() {
        let mut c = ModelClient::new(spec(), Box::new(Fake::new(vec![Ok("nope".into())])), 1);
        assert_eq!(c.complete("p"), Err(CallError::Tripped));
    }

    #[test]
    fn registry_degraded_without_large() {
        let r = Registry::default();
        assert!(r.degraded() && !r.has_large());
    }

    #[test]
    fn seed_chunking_preserves_lines() {
        let chunks = seed_chunks("a\nbbbb\ncc\n", 4);
        assert_eq!(chunks, vec!["a\n", "bbbb\n", "cc\n"]);
    }

    #[test]
    fn small_pool_maps_successful_observations() {
        let mut reg = Registry {
            small: vec![
                ModelClient::new(spec(), Box::new(Fake::new(vec![Ok(ok_body())])), 3),
                ModelClient::new(spec(), Box::new(Fake::new(vec![Ok(ok_body())])), 3),
            ],
            large: None,
        };
        let obs = reg.map_small_pool("line1\nline2\n", 2).unwrap_or_default();
        assert!(obs.contains("hi"));
    }
}
