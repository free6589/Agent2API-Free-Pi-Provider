use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use agent2api_provider_protocol as proto;
use reqwest::{Client, Response};
use serde_json::{json, Value};
use url::Url;

use crate::http::{self, BASE_URL};

pub const CONSENT_VERSION: &str = "4";
const MAX_NETWORK_FAILURES: u8 = 6;
const POLL_STATE_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Debug)]
pub struct DeviceFlow {
    pub session_id: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval_seconds: u64,
}

#[derive(Clone, Debug)]
pub enum TokenPoll {
    Pending {
        retry_after: Option<u64>,
        hint: Option<String>,
    },
    Issued(String),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct MeProfile {
    pub user_id: String,
    pub handle: String,
    pub consent_version: Option<String>,
}

#[derive(Clone, Debug)]
struct PollState {
    interval_seconds: u64,
    next_poll_at: Option<Instant>,
    consecutive_network_failures: u8,
    created_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NetworkRetry {
    Retry { attempt: u8, wait_seconds: u64 },
    Exhausted,
}

fn poll_states() -> &'static Mutex<HashMap<String, PollState>> {
    static STATES: OnceLock<Mutex<HashMap<String, PollState>>> = OnceLock::new();
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_poll_session(session_id: &str, interval_seconds: u64) {
    let Ok(mut states) = poll_states().lock() else {
        return;
    };
    let now = Instant::now();
    states.retain(|_, state| now.saturating_duration_since(state.created_at) < POLL_STATE_TTL);
    states.insert(
        session_id.to_string(),
        PollState {
            interval_seconds: interval_seconds.max(5),
            next_poll_at: None,
            consecutive_network_failures: 0,
            created_at: now,
        },
    );
}

fn poll_gate(session_id: &str) -> Result<Option<u64>, String> {
    let mut states = poll_states()
        .lock()
        .map_err(|_| "free-pi 登录轮询状态锁不可用".to_string())?;
    let state = states
        .entry(session_id.to_string())
        .or_insert_with(|| PollState {
            interval_seconds: 5,
            next_poll_at: None,
            consecutive_network_failures: 0,
            created_at: Instant::now(),
        });

    let now = Instant::now();
    if let Some(deadline) = state.next_poll_at {
        if now < deadline {
            return Ok(Some(rounded_up_seconds(deadline.duration_since(now))));
        }
        state.next_poll_at = None;
    }
    Ok(None)
}

fn poll_interval_seconds(session_id: &str) -> Result<u64, String> {
    let states = poll_states()
        .lock()
        .map_err(|_| "free-pi 登录轮询状态锁不可用".to_string())?;
    Ok(states
        .get(session_id)
        .map(|state| state.interval_seconds)
        .unwrap_or(5)
        .max(5))
}

fn schedule_poll(session_id: &str, wait_seconds: u64) -> Result<(), String> {
    let mut states = poll_states()
        .lock()
        .map_err(|_| "free-pi 登录轮询状态锁不可用".to_string())?;
    let state = states
        .entry(session_id.to_string())
        .or_insert_with(|| PollState {
            interval_seconds: 5,
            next_poll_at: None,
            consecutive_network_failures: 0,
            created_at: Instant::now(),
        });
    state.next_poll_at = Some(Instant::now() + Duration::from_secs(wait_seconds.max(5)));
    Ok(())
}

fn reset_network_failures(session_id: &str) {
    if let Ok(mut states) = poll_states().lock() {
        if let Some(state) = states.get_mut(session_id) {
            state.consecutive_network_failures = 0;
        }
    }
}

fn record_network_failure(session_id: &str) -> Result<NetworkRetry, String> {
    let mut states = poll_states()
        .lock()
        .map_err(|_| "free-pi 登录轮询状态锁不可用".to_string())?;

    let (attempt, wait_seconds) = {
        let state = states
            .entry(session_id.to_string())
            .or_insert_with(|| PollState {
                interval_seconds: 5,
                next_poll_at: None,
                consecutive_network_failures: 0,
                created_at: Instant::now(),
            });
        state.consecutive_network_failures = state.consecutive_network_failures.saturating_add(1);
        (
            state.consecutive_network_failures,
            state.interval_seconds.max(5),
        )
    };

    if attempt >= MAX_NETWORK_FAILURES {
        states.remove(session_id);
        return Ok(NetworkRetry::Exhausted);
    }

    if let Some(state) = states.get_mut(session_id) {
        state.next_poll_at = Some(Instant::now() + Duration::from_secs(wait_seconds));
    }

    Ok(NetworkRetry::Retry {
        attempt,
        wait_seconds,
    })
}

pub fn clear_poll_session(session_id: &str) {
    if let Ok(mut states) = poll_states().lock() {
        states.remove(session_id);
    }
}

fn rounded_up_seconds(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() > 0))
}

fn effective_poll_delay(interval_seconds: u64, retry_after: Option<u64>) -> u64 {
    interval_seconds.max(retry_after.unwrap_or(0)).max(5)
}

fn parse_retry_after(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .map(|seconds| seconds.ceil() as u64)
    })
}

async fn post_json_with_network_retry(
    client: &Client,
    path: &str,
    body: &Value,
    operation: &str,
) -> Result<Response, String> {
    for attempt in 1..=4_u64 {
        match client
            .post(format!("{BASE_URL}{path}"))
            .json(body)
            .send()
            .await
        {
            Ok(response) => return Ok(response),
            Err(error) if attempt < 4 => {
                tokio::time::sleep(Duration::from_millis(1_500 * attempt)).await;
                agent2api_provider_sdk::log_warn!(
                    "{operation}网络请求失败，第 {attempt}/4 次后重试：{error}"
                );
            }
            Err(error) => {
                return Err(format!("{operation}时网络失败，已重试 4 次：{error}"));
            }
        }
    }

    Err(format!("{operation}失败：超过网络重试次数"))
}

pub async fn start_device_flow(client: &Client) -> Result<DeviceFlow, String> {
    let response = post_json_with_network_retry(
        client,
        "/auth/github/device",
        &json!({}),
        "启动 GitHub 设备授权",
    )
    .await?;

    if !response.status().is_success() {
        return Err(http::response_error(response, "启动 GitHub 设备授权").await);
    }

    let value: Value = response
        .json()
        .await
        .map_err(|error| format!("解析 GitHub 设备授权响应失败：{error}"))?;

    let session_id = required_string(&value, "session_id")?;
    let user_code = required_string(&value, "user_code")?;
    let verification_uri = required_string(&value, "verification_uri")?;
    let interval_seconds = value
        .get("interval")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .max(5);

    validate_verification_uri(&verification_uri)?;

    Ok(DeviceFlow {
        session_id,
        user_code,
        verification_uri,
        interval_seconds,
    })
}

pub async fn submit_consent(client: &Client, session_id: &str) -> Result<(), String> {
    let response = post_json_with_network_retry(
        client,
        "/auth/consent",
        &json!({
            "session_id": session_id,
            "consent_version": CONSENT_VERSION
        }),
        "提交 free-pi 使用条款同意状态",
    )
    .await?;

    if !response.status().is_success() {
        return Err(http::response_error(response, "提交 free-pi 使用条款同意状态").await);
    }
    Ok(())
}

pub async fn poll_token(client: &Client, session_id: &str) -> Result<TokenPoll, String> {
    if let Some(wait_seconds) = poll_gate(session_id)? {
        return Ok(TokenPoll::Pending {
            retry_after: Some(wait_seconds),
            hint: Some(format!(
                "free-pi/GitHub 要求降低轮询频率，已等待间隔；约 {wait_seconds} 秒后再次检查。"
            )),
        });
    }

    let response = match client
        .post(format!("{BASE_URL}/auth/token"))
        .json(&json!({ "session_id": session_id }))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return match record_network_failure(session_id)? {
                NetworkRetry::Retry {
                    attempt,
                    wait_seconds,
                } => Ok(TokenPoll::Pending {
                    retry_after: Some(wait_seconds),
                    hint: Some(format!(
                        "free-pi 登录轮询遇到临时网络错误，连续失败 {attempt}/{} 次；约 {wait_seconds} 秒后自动重试。错误：{error}",
                        MAX_NETWORK_FAILURES
                    )),
                }),
                NetworkRetry::Exhausted => {
                    Ok(TokenPoll::Failed(format!(
                        "free-pi 登录轮询连续发生 {} 次网络错误，已停止重试：{error}",
                        MAX_NETWORK_FAILURES
                    )))
                }
            };
        }
    };

    reset_network_failures(session_id);

    let status = response.status();
    let body = response.json::<Value>().await.unwrap_or(Value::Null);

    if status.as_u16() == 403
        && body.get("code").and_then(Value::as_str) == Some("consent_required")
    {
        clear_poll_session(session_id);
        return Ok(TokenPoll::Failed(
            "free-pi 要求重新确认使用条款。请重新发起登录并阅读条款说明。".to_string(),
        ));
    }

    if !status.is_success() {
        clear_poll_session(session_id);
        let message = http::error_message(&body);
        return Ok(TokenPoll::Failed(format!(
            "free-pi 登录轮询失败（HTTP {}）：{message}",
            status.as_u16()
        )));
    }

    if let Some(token) = body.get("token").and_then(Value::as_str) {
        if token.trim().is_empty() {
            clear_poll_session(session_id);
            return Ok(TokenPoll::Failed("free-pi 返回了空 JWT".to_string()));
        }
        clear_poll_session(session_id);
        return Ok(TokenPoll::Issued(token.trim().to_string()));
    }

    if body.get("status").and_then(Value::as_str) == Some("pending") {
        let retry_after = body.get("retry_after").and_then(parse_retry_after);
        let interval_seconds = poll_interval_seconds(session_id)?;
        let wait_seconds = effective_poll_delay(interval_seconds, retry_after);
        schedule_poll(session_id, wait_seconds)?;

        return Ok(TokenPoll::Pending {
            retry_after: Some(wait_seconds),
            hint: Some(format!(
                "GitHub 尚未完成授权；下一次请求至少间隔 {wait_seconds} 秒，避免触发 slow_down。"
            )),
        });
    }

    clear_poll_session(session_id);
    Ok(TokenPoll::Failed(format!(
        "free-pi 返回了无法识别的登录状态：{}",
        http::truncate(&body.to_string(), 300)
    )))
}

pub async fn fetch_me(client: &Client, token: &str) -> Result<MeProfile, String> {
    let response = client
        .get(format!("{BASE_URL}/me"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| format!("验证 free-pi 登录状态失败：{error}"))?;

    if !response.status().is_success() {
        return Err(http::response_error(response, "验证 free-pi 登录状态").await);
    }

    let body = response
        .json::<Value>()
        .await
        .map_err(|error| format!("解析 free-pi 账号信息失败：{error}"))?;

    let user_id = required_string(&body, "user_id")?;
    let handle = body
        .get("handle")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("free-pi user")
        .to_string();

    let consent_version = body
        .get("consent_version")
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(MeProfile {
        user_id,
        handle,
        consent_version,
    })
}

pub fn normalized_account(token: String, profile: MeProfile) -> proto::NormalizedAccount {
    let session_id = uuid::Uuid::new_v4().to_string();
    let identity = profile.user_id.clone();
    let secret = json!({
        "token": token,
        "session_id": session_id,
        "user_id": profile.user_id,
        "handle": profile.handle
    });
    let public = account_view(&secret);

    proto::NormalizedAccount {
        secret,
        public,
        identity,
        desktop: false,
    }
}

pub fn account_view(secret: &Value) -> Value {
    let token = secret.get("token").and_then(Value::as_str).unwrap_or("");
    let tail = token
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    json!({
        "name": secret.get("handle").and_then(Value::as_str).unwrap_or("free-pi account"),
        "uid": secret.get("user_id").and_then(Value::as_str).unwrap_or(""),
        "tokenTail": tail
    })
}

fn required_string(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("free-pi 响应缺少有效字段 {key}"))
}

fn validate_verification_uri(uri: &str) -> Result<(), String> {
    let parsed = Url::parse(uri)
        .map_err(|error| format!("free-pi 返回了无效的 GitHub 授权地址：{error}"))?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("github.com") {
        return Err(
            "free-pi 返回的设备授权地址不是受信任的 https://github.com 地址，已拒绝打开"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_github_https_device_urls_are_accepted() {
        assert!(validate_verification_uri("https://github.com/login/device").is_ok());
        assert!(validate_verification_uri("http://github.com/login/device").is_err());
        assert!(validate_verification_uri("https://evil.example/login").is_err());
    }

    #[test]
    fn public_account_view_never_contains_the_jwt() {
        let secret = json!({
            "token": "secret.jwt.value",
            "session_id": "session-1",
            "user_id": "user-1",
            "handle": "test-user"
        });
        let public = account_view(&secret);
        assert_eq!(
            public.get("tokenTail").and_then(Value::as_str),
            Some("alue")
        );
        assert!(public.get("token").is_none());
        assert!(public.get("session_id").is_none());
    }

    #[test]
    fn server_slow_down_interval_is_never_shortened() {
        assert_eq!(effective_poll_delay(5, None), 5);
        assert_eq!(effective_poll_delay(5, Some(30)), 30);
        assert_eq!(effective_poll_delay(15, Some(10)), 15);
        assert_eq!(effective_poll_delay(2, Some(1)), 5);
    }

    #[test]
    fn poll_delay_rounds_up_instead_of_polling_early() {
        assert_eq!(rounded_up_seconds(Duration::from_millis(1_500)), 2);
        assert_eq!(rounded_up_seconds(Duration::from_secs(3)), 3);
    }

    #[test]
    fn retry_after_accepts_integer_and_fractional_seconds() {
        assert_eq!(parse_retry_after(&json!(30)), Some(30));
        assert_eq!(parse_retry_after(&json!(2.1)), Some(3));
        assert_eq!(parse_retry_after(&json!(0)), Some(0));
        assert_eq!(parse_retry_after(&json!(-1)), None);
    }

    #[test]
    fn network_retries_stop_after_six_consecutive_failures() {
        assert_eq!(next_network_failure_count(0), Some(1));
        assert_eq!(next_network_failure_count(4), Some(5));
        assert_eq!(next_network_failure_count(5), None);
    }
}

#[cfg(test)]
fn next_network_failure_count(current: u8) -> Option<u8> {
    let next = current.saturating_add(1);
    (next < MAX_NETWORK_FAILURES).then_some(next)
}
