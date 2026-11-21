//! Free Pi（api.freepi.ai）独立 WASM 提供商。
//!
//! 上游形态取证自官方 CLI（本机 `Agent2API/free-pi-cli` 检出）：
//!   · 对话是标准 OpenAI Chat Completions：`POST https://api.freepi.ai/v1/chat/completions`，
//!     JWT 走 `Authorization: Bearer <jwt>`，模型 id 用目录里的 id 原样发送；
//!   · 目录是根路径 `GET /client-version`（`src/update-check.ts`），`models[].{id,name}`，
//!     旧服务端可只带单值 `model`；
//!   · 余额是 `GET /me/stats`（`src/usage-tool.ts`），免费额度按百分比 / credits 展示，
//!     上游明令不展示美元金额；
//!   · 上游有反代识别，必须带 CLI 形状的 `x-client-version`（低于 `min` 回 426）
//!     与 `x-session-id`（并发会话闸门）；没有会话 id 就不发，绝不本地编造。
//!
//! 能力位与上游证据一致：chat / model_refresh / usage 为真；refresh 为假
//! （CLI 只有设备授权三步与 `/me` 探活，没有 refresh 路由），因此不声明续期。
//! 每次调用都是新实例，不保存跨调用状态。

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use agent2api_wasm_sdk::{self as sdk, http, json, Error, Result, Value};

mod account;
mod chat;
mod models;
mod usage;

pub const ID: &str = "free-pi";
pub const LABEL: &str = "Free Pi";
pub const BASE: &str = "https://api.freepi.ai";
pub const CLIENT_VERSION_PATH: &str = "/client-version";
pub const STATS_PATH: &str = "/me/stats";
pub const CHAT_PATH: &str = "/v1/chat/completions";

/// 上游强制更新闸门的读数是 `/client-version` 的 `latest`；静态兜底按 2026-11-14 实测值，
/// 目录刷新成功后用线上 `latest` 覆盖。
pub const FALLBACK_CLIENT_VERSION: &str = "0.6.21";

/// 官方客户端在 linux/x64 上实际发出的 UA 形状（`pi (<platform> <release>; <arch>)`）。
const USER_AGENT: &str = "pi (linux 7.2.11-arch1-1; x64)";

fn metadata() -> Value {
    json!({
        "id": ID,
        "label": LABEL,
        "order": 40,
        "capabilities": {
            "chat": true,
            "stateful": false,
            "refresh": false,
            "model_refresh": true,
            "usage": true,
            "web_login": false,
            "checkin": false,
            "claims": false,
            "anonymous_default_session": false,
            "env_credentials": false,
            "supports_default_model": false,
            "sse_model_rewrite": false,
            "sse_strip_newline_chunks": false
        },
        "login_methods": [
            { "kind": "token", "label": "登录 JWT", "placeholder": "粘贴 free-pi 登录后的 JWT（~/.free-pi/agent/credentials.json）" }
        ],
        "account_features": { "usage": true, "credit_detail": true, "edition": false,
            "identifier": ["userId"], "expiry": [] },
        "default_models": models::fallback()
    })
}

/// 逐个请求复用的伪装头；`session` 只透传调用方带来的值，构造不出就不发。
fn request_headers(token: &str, session: &str, client_version: &str) -> Value {
    let mut headers = json!({
        "accept": "application/json",
        "x-stainless-lang": "js",
        "x-stainless-package-version": "4.104.0",
        "x-stainless-os": "Linux",
        "x-stainless-arch": "x64",
        "x-stainless-runtime": "node",
        "x-stainless-runtime-version": "v22.14.0",
        "x-stainless-retry-count": "0",
        "authorization": format!("Bearer {token}"),
        "user-agent": USER_AGENT,
        "accept-language": "*",
        "sec-fetch-mode": "cors"
    });
    if !session.is_empty() {
        headers["x-session-id"] = json!(session);
    }
    headers["x-client-version"] = json!(client_version);
    headers
}

fn header(headers: &Value, name: &str) -> String {
    headers
        .as_object()
        .into_iter()
        .flatten()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, value)| value.as_str())
        .unwrap_or("")
        .to_string()
}

fn error_message(body: &[u8]) -> (String, String) {
    let parsed = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    let code = parsed["code"]
        .as_str()
        .or_else(|| parsed["error"]["code"].as_str())
        .unwrap_or("")
        .to_string();
    let message = parsed["message"]
        .as_str()
        .or_else(|| parsed["error"]["message"].as_str())
        .or_else(|| parsed["error"].as_str())
        .unwrap_or("")
        .chars()
        .take(500)
        .collect::<String>();
    (code, message)
}

/// 上游错误信封按 provider-adaptation 四档归档：401 凭据、429 额度/速率、其余照原样透传。
fn upstream_error(status: u16, body: &[u8]) -> Error {
    let (code, message) = error_message(body);
    let fallback = if message.is_empty() {
        format!("Free Pi 返回 HTTP {status}")
    } else {
        message
    };
    let mut error = Error::http(status, fallback);
    if !code.is_empty() {
        error = error.with_code(format!("free_pi_{code}"));
    }
    error
}

fn handle(method: &str, params: Value) -> Result<Value> {
    match method {
        "init" => Ok(json!([metadata()])),
        "provider.info" => Ok(metadata()),
        "ping" => Ok(json!(true)),
        "account.normalize" => account::normalize(&params),
        "account.view" => account::view(&params["secret"]),
        "account.identity" => Ok(json!(account::identity(&params["secret"])?)),
        "token.ensure" => Ok(json!(account::token(&params["secret"]))),
        "token.refresh" => Err(Error::coded(
            "Free Pi 只有 90 天登录 JWT，上游没有续期路由，请重新登录后粘贴新 JWT",
            "free_pi_no_refresh",
        )),
        "token.expiring" => Ok(json!(false)),
        "models.list" => Ok(models::list()),
        "models.refresh" => models::refresh(&params),
        "usage.query" => usage::query(),
        "conversation" => {
            chat::forward(params)?;
            Ok(Value::Null)
        }
        "classify" => {
            let status = params["status"].as_u64().unwrap_or(502) as u16;
            Ok(json!({ "kind": match status {
                401 => "token_expired",
                429 => "quota_limited",
                409 => "concurrent_session",
                _ => "fatal",
            } }))
        }
        "is_error_response" => Ok(json!(params["status"].as_u64().unwrap_or(502) >= 400)),
        "error.policy" => Ok(json!({ "stop": false })),
        "retry.decision" => Ok(json!({ "action": "default" })),
        "retry_advice" => Ok(Value::Null),
        "cooldown_models" => Ok(json!({ "models": [] })),
        "images.capabilities" => Ok(json!({ "generation": false, "editing": false })),
        _ => Err(Error::unsupported()),
    }
}

sdk::export_provider!(handle);

/// HEAD/GET 的公共响应收尾：非 2xx 统一转成带分类的错误。
fn ensure_ok(response: http::Response) -> Result<http::Response> {
    if (200..300).contains(&response.status) {
        return Ok(response);
    }
    let status = response.status;
    let content_type = header(&response.headers, "content-type").to_ascii_lowercase();
    let bytes = response.bytes(1024 * 1024)?;
    if content_type.contains("text/html") {
        return Err(
            Error::http(403, "Free Pi 返回网页验证，请在官方客户端确认账号状态")
                .with_code("free_pi_challenge_required"),
        );
    }
    Err(upstream_error(status, &bytes))
}

/// 入站会话 id 归一化：trim；空或超 256 字符视为没有会话 id（与网关 `session_of` 同口径）。
fn session_of(raw: &str) -> String {
    let value = raw.trim();
    if value.is_empty() || value.chars().count() > 256 {
        return String::new();
    }
    value.to_string()
}

fn client_version() -> String {
    FALLBACK_CLIENT_VERSION.to_string()
}
