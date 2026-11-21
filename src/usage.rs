//! 余额：`GET /me/stats`（官方 CLI 的 usage 工具同一条接口）。
//! 展示口径照抄上游：免费额度只给百分比 / credits，美元字段只用于算百分比，
//! 不进入展示串，也不留在 raw 里。

use agent2api_wasm_sdk::{self as sdk, http, json, Error, Result, Value};

use crate::{client_version, ensure_ok, request_headers, BASE, ID, STATS_PATH};

fn number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str()?.trim().parse().ok())
        })
        .filter(|value| value.is_finite())
}

fn compact(value: f64) -> String {
    if value >= 10_000.0 {
        format!("{:.1} 万", value / 10_000.0)
    } else if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn iso_millis(value: &str) -> Option<i64> {
    // 只接受 `YYYY-MM-DDTHH:MM:SS(.sss)Z` 这一种上游实际形状，不猜其他时区写法。
    let bytes = value.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| value.get(range)?.parse::<i64>().ok();
    let year = number(0..4)?;
    let month = number(5..7)?;
    let day = number(8..10)?;
    let hour = number(11..13)?;
    let minute = number(14..16)?;
    let second = number(17..19)?;
    if !(1970..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // 公历天数换算（Howard Hinnant days_from_civil），避免为了一个时间戳引入额外依赖。
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400) + hour * 3_600 + minute * 60 + second) * 1_000)
}

fn report(stats: &Value) -> Result<Value> {
    let cap = number(stats.get("cap_usd_today"));
    let spent = number(stats.get("spent_usd_today"));
    let remaining_credits = number(stats.get("remaining_credits_today"));
    let allowance = number(stats.get("allowance_credits"));
    let purchased = number(stats.get("credit_credits"));
    let resets = stats
        .get("refill_at")
        .and_then(Value::as_str)
        .and_then(iso_millis);

    let mut wallets = Vec::new();
    if let (Some(cap), Some(spent)) = (cap, spent) {
        if cap > 0.0 {
            let percent = ((cap - spent) / cap * 100.0).clamp(0.0, 100.0);
            wallets.push(json!({ "type": "entitlement", "balance": percent, "resetsAt": resets }));
        }
    }
    if let Some(remaining) = remaining_credits {
        wallets.push(json!({ "type": "credits", "sourceId": "daily",
            "displayName": "今日免费额度", "balance": remaining, "total": allowance, "resetsAt": resets }));
    }
    if let Some(purchased) = purchased {
        wallets.push(json!({ "type": "credits", "sourceId": "credits",
            "displayName": "购买余额", "balance": purchased }));
    }
    if wallets.is_empty() {
        return Err(Error::http(
            502,
            "Free Pi 未返回可识别的用量读数，不把未知剩余量显示为零",
        ));
    }

    let percent = wallets
        .iter()
        .find(|wallet| wallet["type"] == "entitlement")
        .and_then(|wallet| number(wallet.get("balance")));
    let credits: Option<f64> = {
        let values: Vec<f64> = wallets
            .iter()
            .filter(|wallet| wallet["type"] == "credits")
            .filter_map(|wallet| number(wallet.get("balance")))
            .collect();
        (!values.is_empty()).then(|| values.iter().sum())
    };
    let headline = match (percent, credits) {
        (Some(percent), Some(credits)) => format!("{percent:.0}% · {} credits", compact(credits)),
        (Some(percent), None) => format!("{percent:.0}%"),
        (None, Some(credits)) => format!("{} credits", compact(credits)),
        (None, None) => String::new(),
    };
    let available = match (remaining_credits, purchased) {
        (Some(remaining), Some(purchased)) => Some(remaining + purchased),
        (Some(remaining), None) => Some(remaining),
        (None, Some(purchased)) => Some(purchased),
        (None, None) => None,
    };

    let mut sources = Vec::new();
    for wallet in &wallets {
        if wallet["type"] == "entitlement" {
            sources.push(json!({ "provider": ID, "id": "entitlement", "label": "免费额度余量",
                "unit": "%", "remainingPercent": wallet["balance"], "resetsAt": wallet["resetsAt"] }));
        } else {
            sources.push(json!({ "provider": ID, "id": wallet["sourceId"],
                "label": wallet["displayName"], "unit": "credits",
                "remaining": wallet["balance"], "total": wallet.get("total"),
                "resetsAt": wallet.get("resetsAt") }));
        }
    }

    Ok(json!({ "available": available, "availableView": headline,
        "unit": if percent.is_some() { "%" } else { "credits" },
        "wallets": wallets, "quotaSources": sources,
        "raw": { "tier": stats.get("tier"), "lane": stats.get("lane"),
            "resetsAt": resets, "provider": ID } }))
}

pub fn query() -> Result<Value> {
    let record = sdk::account()?;
    let token = crate::account::token(&record["secret"]);
    if token.is_empty() {
        return Err(Error::http(400, "Free Pi 账号缺少登录 JWT"));
    }
    let url = format!("{BASE}{STATS_PATH}");
    let version = client_version();
    let response = http::open("GET", &url, request_headers(&token, "", &version), None)?;
    let stats = ensure_ok(response)?.json()?;
    report(&stats)
}
