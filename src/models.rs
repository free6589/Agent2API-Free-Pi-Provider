use std::sync::{Mutex, OnceLock};

use agent2api_provider_protocol as proto;
use agent2api_provider_sdk::PluginCtx;
use serde_json::{json, Value};

use crate::http::{self, BASE_URL};

const FALLBACK_CLIENT_VERSION: &str = "0.4.10";

#[derive(Clone, Debug)]
struct CatalogState {
    models: Vec<Value>,
    latest_version: String,
    refreshed_at: i64,
    remote: bool,
}

fn state() -> &'static Mutex<CatalogState> {
    static STATE: OnceLock<Mutex<CatalogState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(CatalogState {
            models: default_models(),
            latest_version: FALLBACK_CLIENT_VERSION.to_string(),
            refreshed_at: 0,
            remote: false,
        })
    })
}

pub fn default_models() -> Vec<Value> {
    vec![json!({
        "id": "deepseek/deepseek-v4-flash",
        "name": "DeepSeek V4 Flash"
    })]
}

pub fn snapshot() -> (Vec<Value>, bool, i64) {
    match state().lock() {
        Ok(guard) => (guard.models.clone(), guard.remote, guard.refreshed_at),
        Err(_) => (default_models(), false, 0),
    }
}

pub fn client_version() -> String {
    match state().lock() {
        Ok(guard) => guard.latest_version.clone(),
        Err(_) => FALLBACK_CLIENT_VERSION.to_string(),
    }
}

pub async fn refresh(ctx: &PluginCtx) -> Result<usize, String> {
    let client = http::client_for(ctx)?;
    let response = client
        .get(format!("{BASE_URL}/client-version"))
        .send()
        .await
        .map_err(|error| format!("获取 free-pi 模型目录失败：{error}"))?;

    if !response.status().is_success() {
        return Err(http::response_error(response, "获取 free-pi 模型目录").await);
    }

    let body = response
        .json::<Value>()
        .await
        .map_err(|error| format!("解析 free-pi 模型目录失败：{error}"))?;

    let latest_version = body
        .get("latest")
        .and_then(Value::as_str)
        .filter(|value| is_semver(value))
        .unwrap_or(FALLBACK_CLIENT_VERSION)
        .to_string();

    let mut models = Vec::new();
    if let Some(items) = body.get("models").and_then(Value::as_array) {
        for item in items {
            let Some(id) = item.get("id").and_then(Value::as_str).map(str::trim) else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(id);
            models.push(json!({ "id": id, "name": name }));
        }
    }

    if models.is_empty() {
        if let Some(model) = body.get("model").and_then(Value::as_str).map(str::trim) {
            if !model.is_empty() {
                models.push(json!({ "id": model, "name": model }));
            }
        }
    }

    if models.is_empty() {
        return Err("free-pi /client-version 没有返回可用模型".to_string());
    }

    let count = models.len();
    if let Ok(mut guard) = state().lock() {
        guard.models = models;
        guard.latest_version = latest_version;
        guard.refreshed_at = http::now_ms();
        guard.remote = true;
    }

    Ok(count)
}

pub fn model_list() -> proto::ModelList {
    let (models, remote, refreshed_at) = snapshot();
    proto::ModelList {
        models,
        meta: proto::CatalogMeta {
            remote,
            refreshed_at,
        },
    }
}

fn is_semver(value: &str) -> bool {
    let mut parts = value.split('.');
    let first = parts.next();
    let second = parts.next();
    let third = parts.next();
    if first.is_none() || second.is_none() || third.is_none() || parts.next().is_some() {
        return false;
    }
    [first, second, third]
        .into_iter()
        .flatten()
        .all(|part| !part.is_empty() && part.chars().all(|character| character.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_three_part_numeric_versions() {
        assert!(is_semver("0.4.10"));
        assert!(!is_semver("0.4"));
        assert!(!is_semver("0.4.10-beta"));
        assert!(!is_semver("latest"));
    }

    #[test]
    fn fallback_catalog_has_the_official_default_model() {
        let models = default_models();
        assert_eq!(
            models.first().and_then(|item| item.get("id")).and_then(Value::as_str),
            Some("deepseek/deepseek-v4-flash")
        );
    }
}
