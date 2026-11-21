//! 对话：标准 OpenAI Chat Completions 直通，body 只强制 `stream = true`（网关要 SSE），
//! 其余字段原样透传；上游非 2xx 在发出任何内容前转成分类错误，不把半截回答拼成成功。

use agent2api_wasm_sdk::{self as sdk, http, json, Error, Result, Value};

use crate::{client_version, ensure_ok, request_headers, session_of, BASE, CHAT_PATH};

pub fn forward(mut params: Value) -> Result<()> {
    let record = sdk::account()?;
    let token = crate::account::token(&record["secret"]);
    if token.is_empty() {
        return Err(Error::http(400, "Free Pi 账号缺少登录 JWT"));
    }
    let session = session_of(params["session"].as_str().unwrap_or(""));
    let mut body = sdk::take_body(&mut params)
        .filter(Value::is_object)
        .ok_or_else(|| Error::http(400, "Free Pi 请求体必须是对象"))?;
    if body["model"].as_str().unwrap_or("").trim().is_empty() {
        return Err(Error::http(400, "Free Pi 需要明确的模型名"));
    }
    body["stream"] = json!(true);
    if body.get("stream_options").is_none() {
        body["stream_options"] = json!({ "include_usage": true });
    }

    let url = format!("{BASE}{CHAT_PATH}");
    let version = client_version();
    let headers = {
        let mut headers = request_headers(&token, &session, &version);
        headers["content-type"] = json!("application/json");
        headers
    };
    let response = http::open("POST", &url, headers, Some(&body))?;
    let mut response = ensure_ok(response)?;
    sdk::stream_head(200)?;
    while let Some(bytes) = response.read()? {
        sdk::stream_bytes(&bytes)?;
    }
    Ok(())
}
