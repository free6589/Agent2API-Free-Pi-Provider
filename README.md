# Agent2API Free Pi Provider

An unofficial, personal-use Agent2API provider plugin for free-pi's OpenAI-compatible API.

This first version targets **Linux x86_64** and implements GitHub Device Flow login, local gateway account storage, dynamic model discovery, Chat Completions forwarding, and read-only usage reporting.

> **Important:** Use only with an account you own and for your own use. Do not share credentials, expose this gateway to other people, pool accounts, evade limits, or remove sponsored content. free-pi's terms say that API-mode responses may contain labeled sponsored content. Prompts, code, files, and responses may be saved and used for training or shared with third parties. Do not submit secrets, confidential, personal, regulated, or sensitive data. Review the current [Terms](https://freepi.ai/terms) and [Privacy Policy](https://freepi.ai/privacy) before enabling the provider.

## Install

Download the Linux x86_64 ZIP from the repository's GitHub Release, then in Agent2API open **Plugins → Install plugin** and upload the ZIP.

The package ID is `agent2api-free-pi-provider`; the provider ID exposed to routing is `free-pi`.

## First login

1. In Agent2API, enable **Free Pi** and choose its GitHub browser login.
2. The login button explicitly records acceptance of free-pi's current consent version after displaying the data-use disclosure.
3. Open the GitHub device URL and enter the displayed code.
4. Agent2API polls until free-pi issues a JWT. The JWT is stored in Agent2API's managed account store and is excluded from the public account view.

## Supported in v0.1.0

- GitHub OAuth device authorization and consent version tracking.
- JWT account storage and identity display.
- Dynamic model list from `GET /client-version`.
- OpenAI-compatible `/v1/chat/completions` requests, including streaming responses passed through by Agent2API.
- Read-only account usage summary from `GET /me/stats` (USD-denominated fields are intentionally omitted).
- Explicit error handling for invalid login and rate limits. No token-refresh endpoint is assumed.

## Deliberate limits

- One personal free-pi account per Agent2API account record. free-pi documents one active CLI/API session per account; this plugin stores one stable session ID per account.
- A 401 is surfaced as a re-login requirement. A 429 is returned to the client and is not treated as a reason to rotate accounts.
- This plugin does not proxy a shared pool, create accounts, bypass limits, filter sponsored response content, or implement check-in/claims.
- The first release package contains only `x86_64-unknown-linux-gnu`. Other platforms are not yet supported.
- The `x-client-version` header follows the latest version reported by `/client-version`; the plugin has a conservative fallback for offline model-catalog checks.

## Build from source

Requires Rust 1.88+.

```bash
cargo test
cargo build --release
```

Create the installable package locally:

```bash
VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d '"' -f2)
TARGET=x86_64-unknown-linux-gnu
mkdir -p dist/stage/bin
cp pack.json dist/stage/pack.json
cp target/release/agent2api-free-pi-provider dist/stage/bin/
(cd dist/stage && zip -qr "../agent2api-free-pi-provider-${VERSION}-${TARGET}.zip" pack.json bin)
```

## Upstream references

- [free-pi-cli source](https://github.com/dennisonbertram/free-pi-cli)
- [free-pi terms](https://freepi.ai/terms)
- [Agent2API](https://github.com/aimod-cc/Agent2API)
- [Agent2Providers SDK and protocol](https://github.com/ProvidersPlugin/Agent2Providers)

## License

AGPL-3.0-only. This repository is an independent, unofficial integration and is not affiliated with or endorsed by free-pi.
