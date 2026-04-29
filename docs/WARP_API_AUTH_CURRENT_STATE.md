# Warp API and Auth Current State

This document describes the current API/auth behavior in this repository. It is an inventory of what already exists: where the app calls Warp-hosted services, how Warp auth is represented, and which current call shapes a non-Warp backend would need to implement to support existing features.

This is not a proposal for new auth behavior or endpoint design.

## Server roots and endpoint configuration

Warp's server locations are centralized in `ChannelState` and `WarpServerConfig`.

- `crates/warp_core/src/channel/config.rs`
  - `WarpServerConfig::production()` sets:
    - `server_root_url = https://app.warp.dev`
    - `rtc_server_url = wss://rtc.app.warp.dev/graphql/v2`
    - `session_sharing_server_url = wss://sessions.app.warp.dev`
    - `firebase_auth_api_key = ...`
- `crates/warp_core/src/channel/state.rs`
  - `ChannelState::server_root_url()` is the base for main HTTP and GraphQL calls.
  - `ChannelState::ws_server_url()` is the GraphQL subscription websocket URL.
  - `ChannelState::rtc_http_url()` derives an HTTP origin from `ws_server_url()` for RTC-backed HTTP/SSE routes.
  - `ChannelState::session_sharing_server_url()` is the websocket base for terminal session sharing.
  - `ChannelState::firebase_api_key()` is used for Firebase token exchange.
- `crates/warp_cli/src/lib.rs`
  - Hidden runtime overrides already exist:
    - `--server-root-url` / `WARP_SERVER_ROOT_URL`
    - `--ws-server-url` / `WARP_WS_SERVER_URL`
    - `--session-sharing-server-url` / `WARP_SESSION_SHARING_SERVER_URL`
- `app/src/lib.rs`
  - Applies those overrides during startup only when `ChannelState::channel().allows_server_url_overrides()` is true.

Important distinction: `server_root_url` is broad. It is used by account/auth, GraphQL, AI REST/SSE, version, client login, and several generated/public API helpers. It is not currently an AI-only endpoint setting.

## Warp credential model

Warp user auth is represented by `app/src/auth/credentials.rs`.

### Credentials

`Credentials` is the long-lived/current auth state:

- `Firebase(FirebaseAuthTokens)`
  - Normal logged-in or anonymous Firebase-backed Warp credentials.
  - Contains an ID token and refresh token.
- `ApiKey { key, owner_type }`
  - Warp API key for direct server auth.
  - This is not provider BYOK; it authenticates to Warp's server.
- `SessionCookie`
  - Ambient browser-session cookie auth.
  - Does not produce an auth header.
- `Test`
  - Test/integration/skip-login credentials behind cfg flags.
  - Does not produce an auth header.

### AuthToken

`AuthToken` is the short-lived/request auth form:

- `AuthToken::Firebase(String)`
- `AuthToken::ApiKey(String)`
- `AuthToken::NoAuth`

`AuthToken::as_bearer_token()` returns a token string for Firebase and Warp API keys, and `None` for `NoAuth`. Callers that use it attach `Authorization: Bearer <token>`.

### Token refresh

`app/src/server/server_api/auth.rs::AuthClient::get_or_refresh_access_token()` is the central existing auth helper.

Current behavior:

- If `skip_login` is enabled, authenticated requests fail.
- If no credentials are present, it returns an error: `Attempted to retrieve access token when user is logged out`.
- `Credentials::ApiKey` returns `AuthToken::ApiKey`.
- `Credentials::Firebase` returns the cached ID token unless it expires within five minutes.
- Expiring Firebase credentials are refreshed through Firebase's REST endpoints using `ChannelState::firebase_api_key()`.
- If direct Firebase token refresh fails, the code falls back to a Warp-server proxy URL built from `ChannelState::server_root_url()`.
- Successful refresh updates `AuthState` and emits `ServerApiEvent::AccessTokenRefreshed { token }`.
- Denied refresh emits `ServerApiEvent::NeedsReauth`.
- `Credentials::SessionCookie` and test credentials return `AuthToken::NoAuth`.

`ServerApiEvent::AccessTokenRefreshed` is redacted in `Debug`.

## GraphQL calls

GraphQL transport lives in `crates/graphql/src/client.rs`.

All normal app GraphQL calls go through `app/src/server/server_api.rs::ServerApi::send_graphql_request()`.

Current behavior:

1. Calls `get_or_refresh_access_token()`.
2. Builds `RequestOptions { auth_token: auth_token.bearer_token(), ... }`.
3. `crates/graphql/src/client.rs::build_graphql_request()` posts to:
   - `{ChannelState::server_root_url()}/graphql/v2?op=<operation>`
4. If `auth_token` is present, it attaches `Authorization: Bearer <token>`.
5. It may also attach ambient-agent headers from `ambient_agent_headers()`.

Because `send_graphql_request()` always calls `get_or_refresh_access_token()`, most GraphQL operations require existing Warp credentials and fail while logged out.

### Main GraphQL call-site groups

These modules call `send_graphql_request()` and therefore use Warp auth by default:

- `app/src/server/server_api/auth.rs`
  - user properties/settings, onboarding state, privacy settings sync, conversation usage, Warp API key management.
- `app/src/server/server_api/workspace.rs`
  - workspace/team membership and workspace state.
- `app/src/server/server_api/team.rs`
  - team, billing/workspace policy, member/admin operations.
- `app/src/server/server_api/object.rs`
  - Warp Drive/cloud objects, folders, workflows, object sync.
- `app/src/server/server_api/block.rs`
  - saved/shared blocks and block metadata.
- `app/src/server/server_api/integrations.rs`
  - integrations and GitHub connection state.
- `app/src/server/server_api/managed_secrets.rs`
  - managed secret CRUD and related secret metadata.
- `app/src/server/server_api/referral.rs`
  - referral data.
- `app/src/server/server_api/ai.rs`
  - many AI operations, including model metadata, command generation, dialogue generation, code embeddings, cloud/ambient agent task operations, feedback/refunds, and conversation operations.

### Existing GraphQL exceptions

There are a few GraphQL paths that do not use `send_graphql_request()`:

- `create_anonymous_user()` in `server_api/auth.rs`
  - Sends the `CreateAnonymousUser` mutation with `default_request_options()` and no prior `get_or_refresh_access_token()`.
- `fetch_user_properties(auth_token)` in `server_api/auth.rs`
  - Sends `GetUser` directly with an optional token supplied by the caller.
  - Used after credential exchange.
- `get_free_available_models()` in `server_api/ai.rs`
  - Commented as a public resolver.
  - Sends unauthenticated if token lookup fails.
  - Uses a best-effort bearer token if one is available.

## REST and SSE calls to Warp server

`app/src/server/server_api.rs` also defines helpers for REST-like Warp API calls.

### `/api/v1/*` helpers

These helpers all call `get_or_refresh_access_token()` and attach `Authorization: Bearer <token>` when the auth token is header-based:

- `get_public_api_response(path)`
  - `GET {server_root_url}/api/v1/{path}`
- `post_public_api_response(path, body)`
  - `POST {server_root_url}/api/v1/{path}`
- `patch_public_api_unit(path, body)`
  - `PATCH {server_root_url}/api/v1/{path}`

They also attach ambient-agent headers when available:

- `X-Warp-Ambient-Workload-Token`
- `X-Warp-Cloud-Agent-ID`
- agent source header, when present.

Despite the helper name `public_api`, these requests still use Warp auth.

Current consumers include `app/src/server/server_api/ai.rs` and `app/src/server/server_api/harness_support.rs`, for paths such as:

- `agent/run`
- `agent/runs`
- `agent/runs/{id}`
- `agent/runs/{id}/conversation`
- `agent/tasks/{id}/cancel`
- `agent/events/{run_id}`
- `agent/messages`
- `agent/messages/{message_id}/read`
- `agent/artifacts/{artifact_uid}`
- `agent/conversations/{conversation_id}`
- `harness-support/*`

### Agent event SSE stream

`ServerApi::stream_agent_events()` opens an SSE stream to:

- `{ChannelState::rtc_http_url()}/api/v1/agent/events/stream?...`

It calls `get_or_refresh_access_token()` first, attaches bearer auth when available, and attaches ambient-agent headers.

### Login notification

`ServerApi::notify_login()` sends an empty authenticated POST to:

- `{server_root_url}/client/login`

It is best-effort: token lookup or request failure is logged.

### Public/optional REST endpoints

These do not strictly require Warp auth:

- `ServerApi::server_time()`
  - `GET {server_root_url}/current_time`
  - No auth header.
- `ServerApi::fetch_channel_versions()`
  - `GET {server_root_url}/client_version` or `/client_version/daily`
  - Includes `X-Warp-Experiment-ID`/anonymous ID.
  - Authorization is optional: it tries to refresh and attach a token; if that fails, it may send an expired cached token; otherwise it sends unauthenticated.

## Current AI calls

Today, most runtime AI features call Warp-hosted GraphQL, REST, or SSE endpoints. Provider BYOK keys are stored locally, but the main in-app Agent path still sends its request to Warp's `/ai/multi-agent` service.

### BYOK provider key storage

`crates/ai/src/api_keys.rs::ApiKeyManager` stores provider keys in secure storage under `AiApiKeys`.

Stored provider keys:

- Google
- Anthropic
- OpenAI
- OpenRouter
- AWS credentials state for Bedrock-related flows

`ApiKeyManager::api_keys_for_request(include_byo_keys, include_aws_bedrock_credentials)` converts configured provider keys into `warp_multi_agent_api::request::settings::ApiKeys`.

These provider keys are distinct from Warp auth credentials. They do not authenticate the app to Warp's API.

### Multi-agent / Warp Agent

Request construction:

- `app/src/ai/agent/api.rs::RequestParams`
  - Carries model IDs, rules/memory flags, Warp Drive context flag, MCP context, permissions, autonomy/isolation, and optional BYOK provider keys.
- `app/src/ai/agent/api/impl.rs::generate_multi_agent_output()`
  - Converts `RequestParams` to `warp_multi_agent_api::Request`.
  - Inserts BYOK keys into `request.settings.api_keys` when present.

Network call:

- `app/src/server/server_api.rs::generate_multi_agent_output()`
  - Calls `get_or_refresh_access_token()`.
  - Posts protobuf to:
    - `{server_root_url}/ai/multi-agent`
    - `{server_root_url}/ai/passive-suggestions` when the request input is `GeneratePassiveSuggestions`
    - `agent-mode-evals/...` variants in eval builds.
  - Attaches `Authorization: Bearer <token>` when available.
  - Attaches `X-Warp-Ambient-Workload-Token` when available.
  - Reads server-sent events and decodes base64-url-safe protobuf `ResponseEvent` payloads.

Implication for a non-Warp backend: to run the current Agent path without changing the client, a backend must implement the existing `/ai/multi-agent` SSE/protobuf contract and, for passive suggestions, `/ai/passive-suggestions`.

### AI REST endpoints

These methods all currently call `get_or_refresh_access_token()` and attach bearer auth when available:

- `generate_ai_input_suggestions()`
  - `POST {server_root_url}/ai/generate_input_suggestions`
  - Used by Next Command / intelligent autosuggestions.
- `get_relevant_files()`
  - `POST {server_root_url}/ai/relevant_files`
- `generate_am_query_suggestions()`
  - `POST {server_root_url}/ai/generate_am_query_suggestions`
  - Eval builds use `/agent-mode-evals/generate_am_query_suggestions`.
- `predict_am_queries()`
  - `POST {server_root_url}/ai/predict_am_queries`
- `transcribe()`
  - `POST {server_root_url}/ai/transcribe`
- `generate_shared_block_title()`
  - `POST {server_root_url}/ai/generate_block_title`
- `generate_code_review_content()`
  - `POST {server_root_url}/ai/generate_code_review_content`

Implication for a non-Warp backend: existing features using these methods need matching route shape, request/response schema, and error behavior if they are to work without client changes.

### AI GraphQL operations

`app/src/server/server_api/ai.rs` uses authenticated GraphQL for many AI flows, including:

- Natural-language command generation:
  - `GenerateCommands`
- Command metadata generation:
  - `GenerateMetadataForCommand`
- Dialogue generation:
  - `GenerateDialogue`
- Request usage/limit information:
  - `GetRequestLimitInfo`
- Hosted model metadata:
  - `GetFeatureModelChoices`
- Codebase context config:
  - `CodebaseContextConfigQuery`
- Relevant fragments and reranking:
  - `GetRelevantFragmentsQuery`
  - `RerankFragments`
- Merkle tree and embedding sync:
  - `SyncMerkleTree`
  - `UpdateMerkleTree`
  - `PopulateMerkleTreeCache`
  - `GenerateCodeEmbeddings`
- Cloud/ambient agent task and artifact flows:
  - `CreateAgentTask`
  - `UpdateAgentTask`
  - task attachment queries
  - artifact upload target/confirmation
- Conversation operations and feedback/refund operations.

Because these use `send_graphql_request()`, they require Warp credentials unless they use a direct/public exception.

Implication for a non-Warp backend: these features either need a backend that implements the current GraphQL operations at `/graphql/v2`, or the client code needs a different existing/custom API path for each feature.

## Warp Drive and cloud object sync

Most Warp Drive/cloud object operations live in `app/src/server/server_api/object.rs`.

- Regular object fetch/update operations use authenticated GraphQL through `send_graphql_request()`.
- Realtime updates use a GraphQL websocket subscription:
  - `get_warp_drive_updates()`
  - Calls `get_or_refresh_access_token()`.
  - Adds `Authorization: Bearer <token>` into the GraphQL websocket init payload when header auth is available.
  - Connects to `ChannelState::ws_server_url()`.

Implication for a non-Warp backend: Warp Drive parity requires both the relevant GraphQL object operations and the websocket subscription contract.

## Session sharing

Terminal session sharing is separate from the main Warp API and uses `ChannelState::session_sharing_server_url()`.

Relevant files:

- `app/src/terminal/shared_session/mod.rs`
- `app/src/terminal/shared_session/sharer/network.rs`
- `app/src/terminal/shared_session/viewer/network.rs`

Current behavior:

- Join links use `{server_root_url}/session/{session_id}` for web links.
- Websocket connections use `session_sharing_server_url`.
- Sharer/viewer initialization payloads include:
  - `anonymous_id`
  - best-effort `access_token` from `get_or_refresh_access_token().ok().and_then(|token| token.bearer_token())`
- Missing token does not necessarily prevent the websocket from opening; the session-sharing protocol/server decides how to handle the payload.

Implication for a non-Warp backend: session sharing needs the current session-sharing websocket protocol, not just main `server_root_url` HTTP routes.

## Remote server auth

Remote server auth is for Warp Remote Environments, not AI.

Relevant files:

- `app/src/remote_server/auth_context.rs`
- `crates/remote_server/src/auth.rs`
- `crates/remote_server/src/client/mod.rs`
- `crates/remote_server/proto/remote_server.proto`
- `crates/remote_server/src/manager.rs`
- `app/src/remote_server/mod.rs`

Current behavior:

- `RemoteServerAuthContext` supplies:
  - a best-effort Warp bearer token for remote daemon protocol messages;
  - a non-secret identity key used to partition remote daemon socket/PID paths.
- The bearer token is sent in the remote protocol `Initialize` message when available.
- When `ServerApiEvent::AccessTokenRefreshed` fires, `RemoteServerManager::rotate_auth_token()` sends an `Authenticate` notification to connected remote daemons for the current identity.
- The identity key uses the logged-in user ID when the user is logged in and non-anonymous; otherwise it uses the anonymous ID.

This auth does not authenticate SSH itself and does not authenticate provider/BYOK requests.

## Telemetry

Telemetry is not sent through `ServerApi::get_or_refresh_access_token()`.

Relevant file:

- `app/src/server/telemetry/mod.rs`

Current behavior:

- Telemetry events use RudderStack destinations from `ChannelState::rudderstack_ugc_destination()` and `ChannelState::rudderstack_non_ugc_destination()`.
- Requests are sent with RudderStack basic auth:
  - username: destination write key
  - password: empty string
- Telemetry is skipped if privacy settings disable it.
- Release/sandbox gating determines whether events are sent over the network.
- UGC telemetry is separated from non-UGC telemetry and can be dropped depending on privacy settings.

OSS channel state currently has no telemetry config by default.

## OAuth, integrations, and web URLs

Several UI paths open web URLs or GraphQL-backed integration flows rooted at `server_root_url`.

Examples:

- OAuth device flow:
  - `ServerApi::create_oauth_client()` uses:
    - `{server_root_url}/api/v1/oauth/token`
    - `{server_root_url}/api/v1/oauth/device/auth`
- GitHub integration auth URL:
  - `app/src/server/server_api/integrations.rs` can point to `{server_root_url}/oauth/connect/github`.
- Privacy/data management and login web URLs elsewhere in the app use `ChannelState::server_root_url()`.

These are Warp-hosted web/server flows today.

## What currently requires a matching custom API

Without changing client code, a non-Warp backend would need to implement the existing Warp API shapes used by the features it wants to support.

### Required for in-app Warp Agent runtime

- `POST /ai/multi-agent`
  - Protobuf `warp_multi_agent_api::Request`
  - SSE stream of base64-url-safe protobuf `ResponseEvent` payloads.
- `POST /ai/passive-suggestions`
  - Same multi-agent request/response-event style for passive suggestions.

### Required for Active AI and adjacent features

- `POST /ai/generate_input_suggestions`
- `POST /ai/generate_am_query_suggestions`
- `POST /ai/predict_am_queries`
- `POST /ai/relevant_files`
- `POST /ai/generate_block_title`
- `POST /ai/transcribe`
- `POST /ai/generate_code_review_content`

### Required for GraphQL-backed AI features

- `/graphql/v2` operations used by `app/src/server/server_api/ai.rs`, including command generation, command metadata, dialogue, model metadata, code context, embeddings, request usage, cloud/ambient agent tasks, artifacts, and conversation operations.

### Required for Warp Drive / cloud objects

- `/graphql/v2` object/workflow/folder operations.
- `ws_server_url` GraphQL subscription endpoint for Warp Drive updates.

### Required for account/workspace/team/integration features

- `/graphql/v2` operations used by:
  - `auth.rs`
  - `workspace.rs`
  - `team.rs`
  - `integrations.rs`
  - `managed_secrets.rs`
  - `referral.rs`

### Required for public API / cloud agent style flows

- `/api/v1/agent/*`
- `/api/v1/harness-support/*`
- `/api/v1/agent/events/stream` on the RTC HTTP origin.

### Required for session sharing

- Session-sharing websocket routes under `session_sharing_server_url`, including create/join/resume flows.

## Current logged-out behavior summary

In the current code, logged-out behavior is mostly determined by whether a path calls `get_or_refresh_access_token()`:

- Calls through `send_graphql_request()` fail while logged out.
- `/api/v1/*` helpers fail while logged out.
- AI REST/SSE endpoints fail while logged out.
- Warp Drive websocket subscription fails while logged out.
- Some public/optional paths still work:
  - anonymous user creation;
  - free available models resolver;
  - current time;
  - client version checks;
  - session-sharing payloads can omit access token, subject to server behavior;
  - telemetry does not use Warp auth.

Provider BYOK keys being present locally does not bypass Warp auth for the current server-backed paths. In the current Agent path, provider keys are serialized into the request sent to Warp's `/ai/multi-agent` endpoint; they are not used by the client to call providers directly.
