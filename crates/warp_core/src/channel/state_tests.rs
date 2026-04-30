use crate::channel::Channel;

use super::derive_http_origin_from_ws_url;

#[test]
fn wss_becomes_https_and_strips_path() {
    let got = derive_http_origin_from_ws_url("wss://rtc.app.warp.dev/graphql/v2");
    assert_eq!(got.as_deref(), Some("https://rtc.app.warp.dev"));
}

#[test]
fn ws_becomes_http_and_preserves_port() {
    let got = derive_http_origin_from_ws_url("ws://localhost:8080/graphql/v2");
    assert_eq!(got.as_deref(), Some("http://localhost:8080"));
}

#[test]
fn unparseable_input_returns_none() {
    assert!(derive_http_origin_from_ws_url("not a url").is_none());
    assert!(derive_http_origin_from_ws_url("https://app.warp.dev").is_none());
}

#[test]
fn allows_server_url_overrides_matches_api_key_app_launch_channels() {
    assert!(Channel::Dev.allows_server_url_overrides());
    assert!(Channel::Local.allows_server_url_overrides());
    assert!(Channel::Integration.allows_server_url_overrides());
    assert!(Channel::Oss.allows_server_url_overrides());
    assert!(!Channel::Stable.allows_server_url_overrides());
    assert!(!Channel::Preview.allows_server_url_overrides());
}
