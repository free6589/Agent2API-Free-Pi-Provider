use std::time::Duration;

use agent2api_provider_sdk::PluginCtx;

pub const BASE_URL: &str = "https://api.freepi.ai";

pub fn client(proxy: &str) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .use_rustls_tls()
        .user_agent(concat!(
            "agent2api-free-pi-provider/",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(Duration::from_secs(25));

    let proxy = proxy.trim();
    if proxy.is_empty() || proxy.eq_ignore_ascii_case("direct") {
        builder = builder.no_proxy();
    } else {
        let configured =
            reqwest::Proxy::all(proxy).map_err(|error| format!("网关代理配置无效: {error}"))?;
        builder = builder.proxy(configured);
    }

    builder
        .build()
        .map_err(|error| format!("创建 free-pi HTTP 客户端失败: {error}"))
}

pub fn client_for(ctx: &PluginCtx) -> Result<reqwest::Client, String> {
    client(&ctx.env.proxy)
}

pub async fn response_error(response: reqwest::Response, operation: &str) -> String {
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    let detail = if body.trim().is_empty() {
        String::new()
    } else {
        format!("：{}", truncate(&body, 500))
    };
    format!("{operation}失败（HTTP {status}）{detail}")
}

pub fn error_message(body: &serde_json::Value) -> String {
    body.get("error")
        .and_then(|value| value.get("message").or(Some(value)))
        .and_then(serde_json::Value::as_str)
        .or_else(|| body.get("message").and_then(serde_json::Value::as_str))
        .unwrap_or("free-pi 上游返回了未说明的错误")
        .to_string()
}

pub fn truncate(input: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (index, character) in input.chars().enumerate() {
        if index >= max_chars {
            output.push('…');
            break;
        }
        output.push(character);
    }
    output
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
