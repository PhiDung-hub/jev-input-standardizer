use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use jev_input_standardizer::{StandardizeOptions, StandardizeResult};
use serde::{Deserialize, Serialize};

use super::EditorError;

const MAX_RESPONSE_BYTES: u64 = 512 * 1024;

/// The loopback bridge, reached over one direct HTTP/1.1 connection.
#[derive(Debug)]
pub(super) struct Endpoint {
    pub authority: String,
    pub path: String,
}

#[derive(Serialize)]
struct Request<'a> {
    context: &'a str,
    options: &'a StandardizeOptions,
}

#[derive(Deserialize)]
struct BridgeFailure {
    message: Option<String>,
}

pub(super) fn standardize(
    endpoint: &Endpoint,
    draft: &str,
    options: &StandardizeOptions,
    timeout: Duration,
) -> Result<StandardizeResult, EditorError> {
    let body = serde_json::to_vec(&Request {
        context: draft,
        options,
    })?;
    let mut stream = TcpStream::connect(&endpoint.authority)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let headers = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        endpoint.path,
        endpoint.authority,
        body.len()
    );
    stream.write_all(headers.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush()?;
    let mut bytes = Vec::new();
    stream
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RESPONSE_BYTES {
        return Err(EditorError::Protocol("response is too large".to_owned()));
    }
    parse_response(&bytes)
}

pub(super) fn parse_response(bytes: &[u8]) -> Result<StandardizeResult, EditorError> {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| EditorError::Protocol("missing HTTP header boundary".to_owned()))?;
    let head = std::str::from_utf8(&bytes[..split])
        .map_err(|error| EditorError::Protocol(error.to_string()))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| EditorError::Protocol("missing HTTP status".to_owned()))?;
    let body = &bytes[split + 4..];
    if !(200..300).contains(&status) {
        let message = serde_json::from_slice::<BridgeFailure>(body)
            .ok()
            .and_then(|failure| failure.message)
            .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned());
        return Err(EditorError::Bridge { status, message });
    }
    Ok(serde_json::from_slice(body)?)
}

pub(super) fn parse_endpoint(raw: &str) -> Result<Endpoint, EditorError> {
    let rest = raw.strip_prefix("http://").ok_or_else(|| {
        EditorError::Endpoint("only a loopback http:// URL is supported".to_owned())
    })?;
    let (authority, base_path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() || authority.chars().any(char::is_whitespace) {
        return Err(EditorError::Endpoint(
            "missing or invalid authority".to_owned(),
        ));
    }
    ensure_loopback(authority)?;
    let base_path = base_path.trim_matches('/');
    let path = match base_path {
        "" => "/standardize".to_owned(),
        path if path.ends_with("standardize") => format!("/{path}"),
        path => format!("/{path}/standardize"),
    };
    Ok(Endpoint {
        authority: with_default_port(authority),
        path,
    })
}

fn ensure_loopback(authority: &str) -> Result<(), EditorError> {
    let host = match authority.strip_prefix('[') {
        Some(rest) => rest.split_once(']').map(|(host, _)| host),
        None => Some(
            authority
                .split_once(':')
                .map_or(authority, |(host, _)| host),
        ),
    }
    .ok_or_else(|| EditorError::Endpoint("invalid host".to_owned()))?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if loopback {
        Ok(())
    } else {
        Err(EditorError::Endpoint(
            "bridge must use a loopback host".to_owned(),
        ))
    }
}

fn with_default_port(authority: &str) -> String {
    let has_port = match authority.strip_prefix('[') {
        Some(rest) => rest.contains("]:"),
        None => authority.contains(':'),
    };
    if has_port {
        authority.to_owned()
    } else {
        format!("{authority}:80")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_loopback_endpoint_and_adds_route() {
        let endpoint = parse_endpoint("http://127.0.0.1:8788").unwrap();
        assert_eq!(endpoint.authority, "127.0.0.1:8788");
        assert_eq!(endpoint.path, "/standardize");
        let ipv6 = parse_endpoint("http://[::1]/jev").unwrap();
        assert_eq!(ipv6.authority, "[::1]:80");
        assert_eq!(ipv6.path, "/jev/standardize");
    }

    #[test]
    fn rejects_non_loopback_endpoint() {
        assert!(parse_endpoint("https://example.com").is_err());
        assert!(parse_endpoint("http://example.com:8788").is_err());
    }

    #[test]
    fn surfaces_bridge_failures_with_their_message() {
        let body = br#"{"code":"usage_exhausted","message":"paused"}"#;
        let mut response = b"HTTP/1.1 402 Payment Required\r\n\r\n".to_vec();
        response.extend_from_slice(body);
        let error = parse_response(&response).unwrap_err();
        assert_eq!(error.to_string(), "bridge returned HTTP 402: paused");
    }
}
