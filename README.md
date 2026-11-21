# Agent2API Free Pi Provider（WASM）

Agent2API 的 Free Pi 提供商插件。**只提供 WASM 形态**：宿主从 3.5 起停用原生可执行插件
（非 `.wasm` 的包一律按 `LegacyUnsupported` 拒绝加载），因此本仓库已从原生 cdylib/bin
迁移为单提供商 WASM 包。

| 项 | 值 |
|---|---|
| 包 id | `agent2api-wasm-free-pi` |
| provider id | `free-pi`（存量 id 不改名） |
| 版本 | `0.2.0` |
| 入口 | `entry.wasm32-unknown-unknown` → `free-pi.wasm` |
| ABI | `abi = 1`，导入只允许 `agent2api.call` / `agent2api.read` |
| 沙箱 | 无 WASI；文件、进程、环境变量都不暴露；每次调用新建实例 |

## 能力与依据

| 能力 | 状态 | 依据 |
|---|---|---|
| 对话（含流式） | 已实现，未验收 | `POST https://api.freepi.ai/v1/chat/completions`，JWT 走标准 Bearer |
| 模型目录 | 已实现，未验收 | `GET /client-version` 的 `models[].{id,name}`，静态兜底一条 |
| 余额 | 已实现，未验收 | `GET /me/stats`；只展示百分比与 credits，不展示美元 |
| 续期 | 不适用 | 上游只有 90 天登录 JWT，没有 refresh 路由（`refresh = false`），过期需重新登录 |
| 签到 / 领取 | 不适用 | 上游无对应入口 |
| 网页登录 | 不适用 | 上游登录是 GitHub 设备授权 + 本地写 `~/.free-pi/agent`，宿主侧只做粘贴 JWT |

`wasm.json` 的 `verified` 保持 `false`：编译通过、能安装都不等于能力已验收。

## 目录

```
Cargo.toml     cdylib 包，依赖 agent2api-wasm-sdk（固定 rev）
pack.json      包清单：单 provider、单 wasm 入口
wasm.json      ABI、requires、HTTPS origin、内存与并发上限
src/lib.rs     导出 ABI、握手、业务分派、伪装头与错误归档
src/account.rs 账号归一化 / 公开视图 / 稳定身份
src/models.rs  /client-version 目录与静态兜底
src/usage.rs   /me/stats 余额归一化
src/chat.rs    /v1/chat/completions 直通与 SSE 转发
```

## 构建

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

产物：`target/wasm32-unknown-unknown/release/agent2api_wasm_free_pi.wasm`。

## 打包与安装

包内只需要 `pack.json`、`wasm.json` 和入口模块：

```bash
VERSION=0.2.0
rm -rf dist && mkdir -p dist/free-pi
cp pack.json wasm.json dist/free-pi/
cp target/wasm32-unknown-unknown/release/agent2api_wasm_free_pi.wasm dist/free-pi/free-pi.wasm
(cd dist/free-pi && zip -qr ../agent2api-wasm-free-pi-$VERSION.zip .)
```

安装：Agent2API 3.5 及以上面板「插件 → 从文件安装」选这个 zip。3.4.x 没有 WASM 宿主，
会把 `wasm32-unknown-unknown` 入口当成不支持的包。

## 上游事实（来自官方 CLI `free-pi-cli` 检出）

- 对话：OpenAI Chat Completions，base 必须带 `/v1`（缺 `/v1` 上游 404）；`model` 就是目录里的 id。
- 伪装头：`x-client-version` 被强制更新闸门读取，低于 `min` 回 426；`x-session-id` 是并发会话闸门的判定键，没有就**不发**，不本地编造。
- 余额：`/me/stats` 的 `*_usd` 字段只用于算百分比，不进入展示串，也不留在 `raw`。
- 鉴权：90 天 JWT，`Authorization: Bearer <jwt>`，无签名与续期链路。
