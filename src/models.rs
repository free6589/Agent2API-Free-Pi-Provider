//! 目录：`GET /client-version` 是权威读数，静态兜底只背书 2026-11-14 实测到的那一条。
//! 上游 `id` 就是请求体里的 model 名，不做别名映射。

use agent2api_wasm_sdk::{self as sdk, http, json, Result, Value};

use crate::{
    client_version, ensure_ok, request_headers, BASE, CLIENT_VERSION_PATH, FALLBACK_CLIENT_VERSION,
};

fn entry(id: &str, name: &str, is_default: bool) -> Value {
    json!({ "id": id, "name": name, "kind": "chat", "isDefault": is_default,
        "supportsToolCall": true, "supportsImages": false, "supportsReasoning": false,
        "inputModalities": ["text"] })
}

pub fn fallback() -> Vec<Value> {
    vec![entry("deepseek-v4-flash", "DeepSeek V4 Flash", true)]
}

pub fn list() -> Value {
    json!({ "models": fallback(), "meta": { "remote": false, "refreshed_at": 0 } })
}

fn rows(payload: &Value) -> Vec<Value> {
    let mut models = Vec::new();
    if let Some(list) = payload["models"].as_array() {
        for (index, row) in list.iter().enumerate() {
            let id = row["id"].as_str().unwrap_or("").trim();
            if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
                continue;
            }
            let label = row["display_name"]
                .as_str()
                .or_else(|| row["name"].as_str())
                .unwrap_or(id)
                .chars()
                .take(128)
                .collect::<String>();
            let mut model = entry(id, &label, index == 0);
            model["upstreamKey"] = json!(id);
            if row["free"].as_bool() == Some(true) {
                model["free"] = json!(true);
            }
            if let Some(multiplier) = row.get("usage_x").filter(|value| value.is_number()) {
                model["usageX"] = multiplier.clone();
            }
            models.push(model);
        }
    }
    if models.is_empty() {
        if let Some(single) = payload["model"]
            .as_str()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            models.push(entry(single, single, true));
        }
    }
    models
}

pub fn refresh(params: &Value) -> Result<Value> {
    let record = sdk::account()?;
    let token = crate::account::token(&record["secret"]);
    if token.is_empty() {
        return Ok(sdk::catalog::failed("Free Pi 账号缺少登录 JWT"));
    }
    let url = format!("{BASE}{CLIENT_VERSION_PATH}");
    let version = client_version();
    let response = match http::open("GET", &url, request_headers(&token, "", &version), None) {
        Ok(response) => response,
        Err(error) => return Ok(sdk::catalog::failed(&error.message)),
    };
    let payload = match ensure_ok(response).and_then(http::Response::json) {
        Ok(payload) => payload,
        Err(error) => return Ok(sdk::catalog::failed(&error.message)),
    };
    let models = rows(&payload);
    if models.is_empty() {
        return Ok(sdk::catalog::failed("Free Pi 目录未返回可用模型"));
    }
    // 上游 `latest` 是强制更新闸门的权威读数，成功刷新后由宿主保存目录快照。
    let _ = params;
    let _ = FALLBACK_CLIENT_VERSION;
    Ok(sdk::catalog::refreshed(models))
}
