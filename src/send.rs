//! Sending a report document elsewhere: `covenant push` and `--report-to`.
//!
//! A document is POSTed as JSON to the URL given. The token, when there is
//! one, comes from `COVENANT_TOKEN` and goes in an `Authorization: Bearer`
//! header: never from the command line, and never into a message. The URL is
//! held to one rule, checked before a run starts: `https` anywhere, plain
//! `http` only to this machine (a loopback address or `localhost`), since the
//! token and the document would otherwise cross the network in the clear;
//! and no credentials in it, since a URL is argv.
//!
//! The binary makes no network call unless one of these is asked for. The
//! HTTP client is the `send` feature's (on by default, and not in WASI
//! builds, which have no sockets).

use crate::error::{CovenantError, Result};

/// Where the token is read from.
pub const TOKEN_ENV: &str = "COVENANT_TOKEN";

/// A URL a document may be sent to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    url: String,
    /// Plain http to this machine: no proxy is ever in the way.
    loopback: bool,
}

impl Destination {
    /// Check `url` against the rule: `https` anywhere, `http` only to a
    /// loopback address or `localhost`, and no credentials in it.
    pub fn parse(url: &str) -> Result<Self> {
        let refuse = |why: &str| CovenantError::Usage {
            message: format!("cannot send to {url}: {why}"),
        };
        let Some((scheme, rest)) = url.split_once("://") else {
            return Err(refuse("not an http(s) URL"));
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.contains('@') {
            return Err(refuse(&format!(
                "credentials do not go in a URL, which is argv; set {TOKEN_ENV}"
            )));
        }
        let host = match authority.strip_prefix('[') {
            Some(v6) => v6.split(']').next().unwrap_or_default(),
            None => authority.split(':').next().unwrap_or_default(),
        };
        if host.is_empty() {
            return Err(refuse("no host"));
        }
        let loopback = host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        match scheme.to_ascii_lowercase().as_str() {
            "https" => {}
            "http" if loopback => {}
            "http" => {
                return Err(refuse(
                    "plain http only to this machine (localhost, 127.0.0.1, ::1); use https",
                ))
            }
            _ => return Err(refuse("not an http(s) URL")),
        }
        Ok(Destination {
            url: url.to_string(),
            loopback,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }
}

/// POST `body` (a JSON document) to `to`, with the token from
/// `COVENANT_TOKEN` when it is set. `Err` says why it was not delivered, in
/// words that never include the token.
pub fn post(to: &Destination, body: &[u8]) -> std::result::Result<(), String> {
    post_as(to, body, TOKEN_ENV)
}

/// [`post`], with the token from the variable `token_env` names: each
/// service its own credential.
pub fn post_as(to: &Destination, body: &[u8], token_env: &str) -> std::result::Result<(), String> {
    let token = std::env::var(token_env).ok().filter(|t| !t.is_empty());
    transport::post(to, body, token.as_deref())
}

#[cfg(all(feature = "send", not(target_os = "wasi")))]
mod transport {
    use std::time::Duration;

    use super::Destination;

    pub fn post(to: &Destination, body: &[u8], token: Option<&str>) -> Result<(), String> {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(30)))
            // A proxy the environment names applies to the network, never to
            // this machine.
            .proxy(match to.loopback {
                true => None,
                false => ureq::Proxy::try_from_env(),
            })
            // A redirect is not followed: it could take the document where
            // the URL rule would not.
            .max_redirects(0)
            .user_agent(concat!("covenant/", env!("CARGO_PKG_VERSION")))
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let mut request = agent
            .post(&to.url)
            .header("Content-Type", "application/json");
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        match request.send(body) {
            Ok(response) if response.status().is_success() => Ok(()),
            Ok(response) if response.status().is_redirection() => Err(format!(
                "HTTP {}: a redirect is not followed",
                response.status().as_u16()
            )),
            Ok(response) => Err(format!("HTTP {}", response.status().as_u16())),
            Err(ureq::Error::StatusCode(code)) => Err(format!("HTTP {code}")),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(not(all(feature = "send", not(target_os = "wasi"))))]
mod transport {
    use super::Destination;

    pub fn post(_: &Destination, _: &[u8], _: Option<&str>) -> Result<(), String> {
        Err(if cfg!(target_os = "wasi") {
            "a WASI build cannot send: it has no sockets".to_string()
        } else {
            "this build cannot send: it was built without the `send` feature".to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Destination;

    #[test]
    fn https_goes_anywhere_and_plain_http_only_to_this_machine() {
        for ok in [
            "https://ingest.example.com/v1/runs",
            "HTTPS://ingest.example.com:8443/v1/runs?team=data",
            "http://localhost:8080/runs",
            "http://127.0.0.1/runs",
            "http://127.1.2.3:9/runs",
            "http://[::1]:8080/runs",
        ] {
            assert!(Destination::parse(ok).is_ok(), "{ok}");
        }
        for (bad, why) in [
            ("http://ingest.example.com/v1/runs", "plain http only"),
            ("http://10.0.0.5/runs", "plain http only"),
            ("https://user:secret@ingest.example.com/", "credentials"),
            ("ftp://ingest.example.com/", "not an http(s) URL"),
            ("ingest.example.com/v1/runs", "not an http(s) URL"),
            ("https:///runs", "no host"),
        ] {
            let err = Destination::parse(bad).unwrap_err().to_string();
            assert!(err.contains(why), "{bad}: {err}");
        }
        assert!(
            Destination::parse("http://localhost/runs")
                .unwrap()
                .loopback
        );
        assert!(!Destination::parse("https://example.com").unwrap().loopback);
    }

    /// A build without the client never claims to have sent a document.
    #[cfg(not(all(feature = "send", not(target_os = "wasi"))))]
    #[test]
    fn a_build_without_the_client_refuses_and_says_why() {
        let to = Destination::parse("http://127.0.0.1:9/runs").unwrap();
        let why = super::post(&to, b"{}").unwrap_err();
        assert!(why.contains("without the `send` feature"), "{why}");
    }
}
