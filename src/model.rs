// P2 暴露的客户端 API（complete/熔断/路由）由 P3 ReACT 驱动消费；
// 阶段交付期允许未调用，单测已覆盖超时/坏响应/跳闸。
#![allow(dead_code)]

use std::fmt;
use std::time::Duration;

/// 模型角色：small=粗筛池（Map 并发）/ large=收敛（Reduce 一次）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Small,
    Large,
}

/// 一个模型端点的规格。key 仅从 env/文件解析，绝不入日志。
pub struct ModelSpec {
    pub role: Role,
    pub endpoint: String,
    pub model: String,
    pub key: String,
    pub timeout: Duration,
    pub max_retries: u32,
}

/// 自定义 Debug：脱敏 key，杜绝密钥进日志/报告。
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

/// 传输层错误：区分可重试瞬态与不可重试。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    Timeout,
    Status(u16),
    Network,
    BadBody,
}

impl TransportError {
    /// 仅瞬态错误退避重试；坏响应/状态码无意义重试。
    fn transient(&self) -> bool {
        matches!(self, TransportError::Timeout | TransportError::Network)
    }
}

/// 网络抽象：单测注入假实现，无需联网即可验证超时/坏响应/熔断。
pub trait Transport: Send + Sync {
    fn post(
        &self,
        endpoint: &str,
        key: &str,
        body: &str,
        timeout: Duration,
    ) -> Result<String, TransportError>;
}

/// 熔断计数：连续失败 ≥ 阈值即跳闸，停止 I/O，不空转。
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

/// 单个模型客户端：硬超时 + 退避重试 + 熔断。失败永不挂起。
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

    /// 发一次完成请求：含 ≤max_retries 退避重试；跳闸即拒，绝不空转。
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

/// 注册表：small 池并发粗筛、large 单点收敛；按角色路由。
#[derive(Default)]
pub struct Registry {
    pub small: Vec<ModelClient>,
    pub large: Option<ModelClient>,
}

impl Registry {
    pub fn has_large(&self) -> bool {
        self.large.is_some()
    }
    /// 缺 large(收敛模型)即降级——上层据此回退 AST-only，不阻断。
    pub fn degraded(&self) -> bool {
        self.large.is_none()
    }
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

/// 解析 OpenAI 风格响应；任何缺字段/坏 JSON 返回 None 计入熔断。
fn parse_content(raw: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let c = v.get("choices")?.get(0)?.get("message")?.get("content")?;
    c.as_str().map(str::to_string)
}

/// ureq 阻塞实现：硬超时，绝不无限等待。
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
    use std::cell::RefCell;

    struct Fake {
        seq: RefCell<Vec<Result<String, TransportError>>>,
    }
    impl Fake {
        fn new(seq: Vec<Result<String, TransportError>>) -> Self {
            Self {
                seq: RefCell::new(seq),
            }
        }
    }
    unsafe impl Send for Fake {}
    unsafe impl Sync for Fake {}
    impl Transport for Fake {
        fn post(&self, _: &str, _: &str, _: &str, _: Duration) -> Result<String, TransportError> {
            self.seq
                .borrow_mut()
                .pop()
                .unwrap_or(Err(TransportError::Network))
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
}
