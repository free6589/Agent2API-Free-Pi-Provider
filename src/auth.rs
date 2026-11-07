use std::time::Duration;

use agent2api_provider_protocol as proto;
use reqwest::Client;
use serde_json::{json, Value};
use url::Url;

use crate::http::{self, BASE_URL};

pub const CONSENT_VERSION: &str = "4";

#[derive(Clone, Debug)]
pub struct DeviceFlow {
    pub session_id: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval_seconds: u64,
}

#[derive(Clone, Debug)]
pub enum TokenPoll {
    Pending { retry_after: Option<u64> },
    Issued(String),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct MeProfile {
    pub user_id: String,
    pub handle: String,
    pub consent_version: Option<String>,
}

pub async fn start_device_flow(client: &Client) -> Result<DeviceFlow, String> {
    for attempt in 1..=4_u64 {
        let response = client
            .post(format!("{BASE_URL}/auth/github/device"))
            .json(&json!({}))
            .send()
            .await;

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                if attempt < 4 {
                    tokio::time::sleep(Duration::from_millis(1_500 * attempt)).await;
                    continue;
                }
                return Err(format!("启动 GitHub 设备授权时网络失败：{error}"));
            }
        };

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

        return Ok(DeviceFlow {
            session_id,
            user_code,
            verification_uri,
            interval_seconds,
        });
    }

    Err("启动 GitHub 设备授权失败：超过网络重试次数".to_string())
}

pub async fn submit_consent(client: &Client, session_id: &str) -> Result<(), String> {
    let response = client
        .post(format!("{BASE_URL}/auth/consent"))
        .json(&json!({
            "session_id": session_id,
            "consent_version": CONSENT_VERSION
        }))
        .send()
        .await
        .map_err(|error| format!("提交 free-pi 使用条款同意状态失败：{error}"))?;

    if !response.status().is_success() {
        return Err(http::response_error(response, "提交 free-pi 使用条款同意状态").await);
    }
    Ok(())
}

pub async fn poll_token(client: &Client, session_id: &str) -> Result<TokenPoll, String> {
    let response = client
        .post(format!("{BASE_URL}/auth/token"))
        .json(&json!({ "session_id": session_id }))
        .send()
        .await
        .map_err(|error| format!("轮询 GitHub 设备授权失败：{error}"))?;

    let status = response.status();
    let body = response.json::<Value>().await.unwrap_or(Value::Null);

    if status.as_u16() == 403
        && body.get("code").and_then(Value::as_str) == Some("consent_required")
    {
        return Ok(TokenPoll::Failed(
            "free-pi 要求重新确认使用条款。请重新发起登录并阅读条款说明。".to_string(),
        ));
    }

    if !status.is_success() {
        let message = http::error_message(&body);
        return Ok(TokenPoll::Failed(format!(
            "free-pi 登录轮询失败（HTTP {}）：{message}",
            status.as_u16()
        )));
    }

    if let Some(token) = body.get("token").and_then(Value::as_str) {
        if token.trim().is_empty() {
            return Ok(TokenPoll::Failed("free-pi 返回了空 JWT".to_string()));
        }
        return Ok(TokenPoll::Issued(token.trim().to_string()));
    }

    if body.get("status").and_then(Value::as_str) == Some("pending") {
        let retry_after = body.get("retry_after").and_then(Value::as_u64);
        return Ok(TokenPoll::Pending { retry_after });
    }

    if body.get("code").and_then(Value::as_str) == Some("consent_required") {
        return Ok(TokenPoll::Failed(
            "free-pi 尚未记录使用条款同意状态。请重新发起登录。".to_string(),
        ));
    }

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
}
