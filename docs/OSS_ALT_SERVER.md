# Pointing the OSS Warp build at an alternate server

The `warp-oss` binary (`Channel::Oss`) ships with all the wiring needed to talk
to a self-hosted, Warp-compatible alt server without any Firebase round-trip.
Set two environment variables before launching:

| Variable | Purpose |
| -------- | ------- |
| `WARP_SERVER_ROOT_URL` | Base URL of your alt server (e.g. `https://warp.alt.example`). Used for all REST + GraphQL requests. |
| `WARP_API_KEY` | Opaque bearer token your alt server accepts. Sent as `Authorization: Bearer <token>` on every authenticated request. |

Optional:

| Variable | Purpose |
| -------- | ------- |
| `WARP_WS_SERVER_URL` | WebSocket endpoint for GraphQL subscriptions (e.g. Warp Drive realtime). Defaults are derived from `WARP_SERVER_ROOT_URL` if unset. |
| `WARP_SESSION_SHARING_SERVER_URL` | Session-sharing WS endpoint (optional; leave unset to disable session sharing). |

## Quick start

```bash
export WARP_SERVER_ROOT_URL="https://warp.alt.example"
export WARP_API_KEY="<token-issued-by-your-alt-server>"
warp-oss   # or `cargo run --bin warp-oss`
```

That's the entire integration on the client side. Internally:

- `WARP_API_KEY` is consumed at startup in `app/src/lib.rs::run_internal()` and
  seeds `Credentials::ApiKey` via `AuthState::initialize()`.
- All authenticated requests go through `AuthClient::get_or_refresh_access_token()`
  in `app/src/server/server_api/auth.rs`, which short-circuits the Firebase
  refresh path for `Credentials::ApiKey` and returns `AuthToken::ApiKey(key)`
  unchanged. No call to `securetoken.googleapis.com`.
- The `SkipFirebaseAnonymousUser` feature flag (default-enabled on OSS) skips
  the anonymous-user bootstrap mutation, so first-launch never round-trips
  through Warp's hosted GraphQL either.

## Channel scope

`--api-key` / `WARP_API_KEY` is honored on every channel that already accepts
server URL overrides (Dev, Local, Integration, OSS). Stable and Preview ignore
both, so shipped first-party builds can't be redirected. See
`Channel::allows_server_url_overrides` in `crates/warp_core/src/channel/mod.rs`.

## Out of scope for this minimal recipe

This document describes the bare minimum to point a single OSS build at a
single alt server using a static long-lived bearer token. It deliberately does
NOT cover:

- Refresh-token flows / short-lived bearer credentials.
- In-app login UI for alt servers (the existing API Keys widget can still be
  used to paste a token at runtime).
- Hiding hosted-billing UI surfaces (most are already gated on
  `Channel::Oss` upstream; remaining surfaces will be addressed reactively).
- Local context packaging (rules / MCP / Drive). Alt servers running on the
  same machine can read the user's filesystem directly.

These slices will be layered on only when a concrete user-facing gap is
verified. See issues #14 and #15 for the broader pivot context.
