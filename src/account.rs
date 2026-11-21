//! 账号形态：上游只有一枚 90 天登录 JWT，没有续期材料，公开视图不出现凭据本体。

use agent2api_wasm_sdk::{json, Error, Result, Value};

use crate::ID;

fn text(secret: &Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(value) = secret.get(*key).and_then(Value::as_str) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    secret.as_str().map(str::trim).unwrap_or("").to_string()
}

pub fn token(secret: &Value) -> String {
    text(
        secret,
        &[
            "token",
            "jwt",
            "accessToken",
            "access_token",
            "apiKey",
            "api_key",
        ],
    )
}

fn fingerprint(token: &str) -> String {
    // FNV-1a 64：只用于稳定身份，不可逆，不承载凭据本体。
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in token.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub fn identity(secret: &Value) -> Result<String> {
    let token = token(secret);
    if token.is_empty() {
        return Err(Error::http(400, "Free Pi 账号缺少登录 JWT"));
    }
    Ok(format!("{ID}-{}", fingerprint(&token)))
}

fn tail(token: &str) -> String {
    if token.chars().count() <= 12 {
        return String::new();
    }
    token
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

pub fn view(secret: &Value) -> Result<Value> {
    let token = token(secret);
    let name = {
        let name = text(secret, &["name"]);
        if name.is_empty() {
            "Free Pi 账号".to_string()
        } else {
            name
        }
    };
    Ok(json!({
        "name": name,
        "tokenTail": tail(&token),
        "source": text(secret, &["source"]),
        "canRefresh": false,
        "hasRefreshToken": false,
        "desktop": false,
        "available": !token.is_empty()
    }))
}

pub fn normalize(params: &Value) -> Result<Value> {
    let source = &params["source"];
    let mut secret = match source["kind"].as_str() {
        Some("manual") => {
            let payload = &source["payload"];
            if payload.is_object() {
                payload.clone()
            } else if let Some(input) = payload.as_str() {
                let input = input.trim();
                if input.is_empty() {
                    return Err(Error::http(400, "Free Pi 登录 JWT 不能为空"));
                }
                if input.starts_with('{') {
                    serde_json::from_str::<Value>(input)
                        .map_err(|_| Error::http(400, "Free Pi 凭据 JSON 格式无效"))?
                } else {
                    json!({ "token": input })
                }
            } else {
                return Err(Error::http(400, "Free Pi 账号内容必须为凭据对象或 JWT"));
            }
        }
        _ => return Err(Error::http(400, "Free Pi 只支持粘贴登录 JWT")),
    };
    if !secret.is_object() {
        return Err(Error::http(400, "Free Pi 凭据必须为对象"));
    }
    if secret.get("token").is_none() {
        let token = token(&secret);
        secret["token"] = json!(token);
    }
    if token(&secret).is_empty() {
        return Err(Error::http(400, "Free Pi 账号缺少登录 JWT"));
    }
    if text(&secret, &["name"]).is_empty() {
        secret["name"] = json!("Free Pi 账号");
    }
    let public = view(&secret)?;
    Ok(
        json!({ "identity": identity(&secret)?, "secret": secret, "public": public, "desktop": false }),
    )
}
