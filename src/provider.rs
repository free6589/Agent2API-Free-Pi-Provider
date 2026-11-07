use agent2api_provider_protocol as proto;
use agent2api_provider_sdk::{ChatPlan, PluginCtx, Provider, ProviderMeta, SendFuture};
use serde_json::{json, Value};

use crate::{auth, http, models};

pub const ID: &str = "free-pi";

pub struct FreePiProvider;

impl Provider for FreePiProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new(ID, "Free Pi")
            .with_capabilities(proto::Capabilities {
                chat: true,
                stateful: false,
                refresh: false,
                model_refresh: true,
                usage: true,
                checkin: false,
                claims: false,
                web_login: true,
                sse_model_rewrite: false,
                sse_strip_newline_chunks: false,
                anonymous_default_session: false,
                system_proxy_when_unset: false,
                refresh_uses_account: false,
                supports_default_model: false,
                env_credentials: false,
            })
            .with_login(vec![
                proto::LoginMethod::Browser {
                    button: "我已阅读并同意 free-pi 条款，使用 GitHub 登录".to_string(),
                    hint: Some(
                        "登录会向 free-pi 提交当前同意版本（4）。free-pi 会保存提示词、代码、文件上下文和模型响应，并用于训练、评估与改进；条款允许向第三方分享或出售 traces。部分广告合作方可能接收近期对话摘录用于广告选择。不要提交密码、API Key、私钥、个人信息、商业机密或其他敏感数据。请先阅读 https://freepi.ai/terms 和 https://freepi.ai/privacy。点击此按钮即表示你已阅读并同意这些条款。"
                            .to_string(),
                    ),
                },
                proto::LoginMethod::Token {
                    label: "free-pi JWT".to_string(),
                    placeholder: "粘贴 free-pi credentials.json 中的 token".to_string(),
                    extra_fields: vec![],
                },
            ])
            .with_models(models::default_models())
            .with_account_features(proto::AccountFeatures {
                usage: true,
                identifier: vec!["uid".to_string()],
                ..Default::default()
            })
    }

    fn build_chat<'a>(
        &'a self,
        ctx: &'a PluginCtx,
        req: proto::ChatRequest,
    ) -> SendFuture<'a, ChatPlan> {
        Box::pin(async move {
            let account = ctx
                .store_get(ID, &req.account)
                .await
                .map_err(String::from)?;
            let token = account
                .secret
                .get("token")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "free-pi 账号没有 JWT，请重新登录".to_string())?;
            let session_id = account
                .secret
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "free-pi 账号缺少 session_id，请重新登录".to_string())?;

            let body = rewrite_model(&req.body, &req.wire_model)?;
            let headers = vec![
                ("content-type".to_string(), "application/json".to_string()),
                ("authorization".to_string(), format!("Bearer {token}")),
                ("x-session-id".to_string(), session_id.to_string()),
                ("x-client-version".to_string(), models::client_version()),
            ];

            Ok(ChatPlan::chat(
                format!("{}/v1/chat/completions", http::BASE_URL),
                headers,
                body,
            ))
        })
    }

    fn classify(&self, status: u16, body: &Value) -> proto::ErrorClass {
        let upstream_message = http::error_message(body);
        let message = match status {
            401 | 403 => format!(
                "free-pi 拒绝了当前 JWT 或授权状态（HTTP {status}）。该服务没有在此 Provider 中开放 Token Refresh，请重新登录。上游信息：{upstream_message}"
            ),
            429 => format!(
                "free-pi 限制了当前账号的请求频率或额度（HTTP 429）。本 Provider 不会通过轮换账号规避限制。上游信息：{upstream_message}"
            ),
            _ => format!("free-pi 上游返回 HTTP {status}：{upstream_message}"),
        };

        proto::ErrorClass::Fatal {
            status,
            message,
            upstream_code: body
                .get("error")
                .and_then(|value| value.get("code"))
                .and_then(Value::as_i64),
        }
    }

    fn ensure_token<'a>(
        &'a self,
        ctx: &'a PluginCtx,
        account_id: &'a str,
    ) -> SendFuture<'a, String> {
        Box::pin(async move {
            let account = ctx.store_get(ID, account_id).await.map_err(String::from)?;
            account
                .secret
                .get("token")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .map(str::to_string)
                .ok_or_else(|| "free-pi 账号没有 JWT，请重新登录".to_string())
        })
    }

    fn models<'a>(
        &'a self,
        ctx: &'a PluginCtx,
        _account: &'a str,
    ) -> SendFuture<'a, proto::ModelList> {
        Box::pin(async move {
            if let Err(error) = models::refresh(ctx).await {
                agent2api_provider_sdk::log_warn!(
                    "free-pi 模型目录在线刷新失败，使用最近缓存：{error}"
                );
            }
            Ok(models::model_list())
        })
    }

    fn refresh_models<'a>(
        &'a self,
        ctx: &'a PluginCtx,
        _account: &'a str,
        _force: bool,
    ) -> SendFuture<'a, proto::RefreshOutcome> {
        Box::pin(async move {
            match models::refresh(ctx).await {
                Ok(count) => Ok(proto::RefreshOutcome::refreshed(count)),
                Err(error) => Ok(proto::RefreshOutcome::failed(error)),
            }
        })
    }

    fn usage<'a>(
        &'a self,
        ctx: &'a PluginCtx,
        account_id: &'a str,
    ) -> SendFuture<'a, proto::UsageReport> {
        Box::pin(async move {
            let account = ctx.store_get(ID, account_id).await.map_err(String::from)?;
            let token = account
                .secret
                .get("token")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "free-pi 账号没有 JWT，请重新登录".to_string())?;

            let client = http::client_for(ctx)?;
            let response = client
                .get(format!("{}/me/stats", http::BASE_URL))
                .bearer_auth(token)
                .send()
                .await
                .map_err(|error| format!("查询 free-pi 使用情况失败：{error}"))?;

            if !response.status().is_success() {
                return Err(http::response_error(response, "查询 free-pi 使用情况").await);
            }

            let stats = response
                .json::<Value>()
                .await
                .map_err(|error| format!("解析 free-pi 使用情况失败：{error}"))?;

            let allowance = number(&stats, "allowance_credits");
            let remaining = number(&stats, "remaining_credits_today");
            let purchased = number(&stats, "credit_credits");
            let used = allowance.saturating_sub(remaining);
            let lifetime = stats.get("lifetime").cloned().unwrap_or(Value::Null);

            Ok(json!({
                "plan": stats.get("tier").and_then(Value::as_str).unwrap_or("unknown"),
                "balance": purchased,
                "windows": [{
                    "name": "今日免费额度",
                    "used": used,
                    "limit": allowance,
                    "unit": "credits",
                    "until": stats.get("refill_at").and_then(Value::as_str).unwrap_or("")
                }],
                "lane": stats.get("lane").and_then(Value::as_str).unwrap_or("unknown"),
                "promptTokensToday": number(&stats, "prompt_tokens_today"),
                "completionTokensToday": number(&stats, "completion_tokens_today"),
                "lifetime": {
                    "promptTokens": number(&lifetime, "prompt_tokens"),
                    "completionTokens": number(&lifetime, "completion_tokens")
                }
            }))
        })
    }

    fn normalize_account<'a>(
        &'a self,
        ctx: &'a PluginCtx,
        input: proto::NormalizeInput,
    ) -> SendFuture<'a, proto::NormalizedAccount> {
        Box::pin(async move {
            let token = match input.source {
                proto::NormalizeSource::Manual { payload } => payload
                    .get("token")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| "请输入 free-pi JWT".to_string())?,
                proto::NormalizeSource::FileText { content, .. } => parse_token_file(&content)?,
                proto::NormalizeSource::FileBase64 { .. } => {
                    return Err("第一版暂不支持二进制凭证文件导入".to_string());
                }
                proto::NormalizeSource::Desktop => {
                    return Err("free-pi 没有可供本插件读取的桌面端登录态".to_string());
                }
            };

            let client = http::client_for(ctx)?;
            let profile = auth::fetch_me(&client, &token).await?;
            Ok(auth::normalized_account(token, profile))
        })
    }

    fn account_view(&self, secret: &Value) -> Result<proto::AccountView, String> {
        let token = secret.get("token").and_then(Value::as_str).unwrap_or("");
        let user_id = secret.get("user_id").and_then(Value::as_str).unwrap_or("");
        if token.is_empty() || user_id.is_empty() {
            return Err("free-pi 账号凭证不完整，请重新登录".to_string());
        }
        Ok(auth::account_view(secret))
    }

    fn account_identity(&self, secret: &Value) -> Result<String, String> {
        secret
            .get("user_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "free-pi 账号缺少 user_id".to_string())
    }

    fn login_start<'a>(
        &'a self,
        ctx: &'a PluginCtx,
        method: &'a str,
        _payload: &'a Value,
    ) -> SendFuture<'a, proto::LoginStart> {
        Box::pin(async move {
            if method != "browser" {
                return Err(format!("free-pi 不支持登录方式 {method}"));
            }

            let client = http::client_for(ctx)?;
            let device = auth::start_device_flow(&client).await?;
            auth::submit_consent(&client, &device.session_id).await?;

            Ok(proto::LoginStart {
                flow: device.session_id,
                url: device.verification_uri,
                instructions: format!(
                    "请在 GitHub 页面输入设备验证码：{}。授权完成后 Agent2API 会自动取得 free-pi JWT。",
                    device.user_code
                ),
                poll_interval_ms: device.interval_seconds.saturating_mul(1000),
                external_browser: true,
                allowed_hosts: vec!["github.com".to_string()],
            })
        })
    }

    fn login_poll<'a>(
        &'a self,
        ctx: &'a PluginCtx,
        flow: &'a str,
    ) -> SendFuture<'a, proto::LoginPoll> {
        Box::pin(async move {
            let client = http::client_for(ctx)?;
            match auth::poll_token(&client, flow).await? {
                auth::TokenPoll::Pending { retry_after } => Ok(proto::LoginPoll::Pending {
                    hint: retry_after
                        .map(|seconds| format!("GitHub 尚未完成授权，建议等待 {seconds} 秒后继续")),
                }),
                auth::TokenPoll::Issued(token) => {
                    let profile = auth::fetch_me(&client, &token).await?;
                    if profile
                        .consent_version
                        .as_deref()
                        .is_some_and(|version| version != auth::CONSENT_VERSION)
                    {
                        return Ok(proto::LoginPoll::Failed {
                            message:
                                "free-pi 服务端返回的同意版本与当前版本不一致，请重新发起登录。"
                                    .to_string(),
                        });
                    }
                    Ok(proto::LoginPoll::Done {
                        account: auth::normalized_account(token, profile),
                    })
                }
                auth::TokenPoll::Failed(message) => Ok(proto::LoginPoll::Failed { message }),
            }
        })
    }
}

fn rewrite_model(body: &Value, wire_model: &str) -> Result<Value, String> {
    if wire_model.trim().is_empty() {
        return Err("Agent2API 没有提供 free-pi 的发送模型名".to_string());
    }
    let mut rewritten = body.clone();
    let object = rewritten
        .as_object_mut()
        .ok_or_else(|| "客户端请求体必须是 JSON object".to_string())?;
    object.insert("model".to_string(), Value::String(wire_model.to_string()));
    Ok(rewritten)
}

fn parse_token_file(content: &str) -> Result<String, String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err("凭证文件为空".to_string());
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return value
            .get("token")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .ok_or_else(|| "JSON 凭证文件中没有有效的 token 字段".to_string());
    }
    Ok(trimmed.to_string())
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_only_the_model_field() {
        let body = json!({
            "model": "old-name",
            "messages": [{"role": "user", "content": "hello"}],
            "stream": true
        });
        let rewritten = rewrite_model(&body, "deepseek/deepseek-v4-flash");
        assert!(rewritten.is_ok());
        let rewritten = rewritten.unwrap_or(Value::Null);
        assert_eq!(
            rewritten.get("model").and_then(Value::as_str),
            Some("deepseek/deepseek-v4-flash")
        );
        assert_eq!(rewritten.get("stream").and_then(Value::as_bool), Some(true));
    }

    #[test]
    fn manual_token_file_accepts_raw_token_or_credentials_json() {
        assert_eq!(
            parse_token_file("jwt-value").ok().as_deref(),
            Some("jwt-value")
        );
        assert_eq!(
            parse_token_file(r#"{"token":"jwt-value"}"#).ok().as_deref(),
            Some("jwt-value")
        );
        assert!(parse_token_file(r#"{"other":"value"}"#).is_err());
    }

    #[test]
    fn rate_limits_are_not_account_rotation_errors() {
        let class = FreePiProvider.classify(429, &json!({"error":{"message":"too many requests"}}));
        assert!(matches!(
            class,
            proto::ErrorClass::Fatal { status: 429, .. }
        ));
    }

    #[test]
    fn expired_credentials_are_not_retried_with_the_same_jwt() {
        let class = FreePiProvider.classify(401, &json!({"message":"unauthorized"}));
        assert!(matches!(
            class,
            proto::ErrorClass::Fatal { status: 401, .. }
        ));
    }
}
