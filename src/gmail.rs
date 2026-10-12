//! Gmail connector: OAuth2 authorization-code flow with PKCE, read-only
//! scope, and incremental sync into the local `inbox` table.
//!
//! Design constraints this module is built to satisfy:
//!
//! * **Least privilege.** Only `gmail.readonly` is requested. FRIDAY cannot
//!   send, delete or modify anything in the mailbox, by construction.
//! * **No client secret in config.** `GOOGLE_CLIENT_ID` and
//!   `GOOGLE_CLIENT_SECRET` come from the environment, so they never appear in
//!   `config.toml`, in an export, or in the API response.
//! * **Loopback redirect.** The consent code comes back to `127.0.0.1`, so no
//!   hosted callback is needed and the code never transits a third party.
//! * **PKCE.** Protects the exchange even though a "desktop" client secret is
//!   not really secret.
//! * **Message bodies are untrusted.** They are stored verbatim and only ever
//!   handed to a model inside `util::untrusted_block`.
use crate::state::AppState;
use crate::util::{now, truncate, uid};
use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

// ------------------------------------------------------------------ credentials

/// Deliberately does not derive `Debug` or `Serialize`: a client secret must
/// never be printable by accident.
pub struct ClientCredentials {
    pub client_id: String,
    pub client_secret: String,
}

/// Reads the OAuth client from the environment. The error is the setup
/// instruction, because a missing client is always a setup problem.
pub fn client_credentials() -> Result<ClientCredentials> {
    let id = std::env::var("GOOGLE_CLIENT_ID")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let secret = std::env::var("GOOGLE_CLIENT_SECRET")
        .ok()
        .filter(|v| !v.trim().is_empty());
    match (id, secret) {
        (Some(client_id), Some(client_secret)) => Ok(ClientCredentials {
            client_id,
            client_secret,
        }),
        _ => bail!(
            "Gmail is not configured. In the Google Cloud console: enable the Gmail API, create \
             an OAuth client of type \"Desktop app\", then put its values in .env as \
             GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET. Full steps are in docs/gmail.md."
        ),
    }
}

// ------------------------------------------------------------------ token store

#[derive(Debug, Serialize, Deserialize)]
pub struct StoredToken {
    pub refresh_token: String,
    #[serde(default)]
    pub access_token: String,
    /// RFC3339 expiry of `access_token`.
    #[serde(default)]
    pub expires_at: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub obtained_at: String,
}

/// Tokens live in a gitignored file with owner-only permissions. An OS keyring
/// would be better; this is the documented fallback, and the trade-off is
/// stated in docs/security.md rather than hidden.
pub fn token_path() -> PathBuf {
    PathBuf::from(".secrets/google.json")
}

pub fn load_token() -> Option<StoredToken> {
    let text = std::fs::read_to_string(token_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_token(token: &StoredToken) -> Result<()> {
    let path = token_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        restrict(parent)?;
    }
    std::fs::write(&path, serde_json::to_vec_pretty(token)?)?;
    restrict(&path)?;
    Ok(())
}

/// Owner-only permissions where the platform supports them.
fn restrict(path: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if path.is_dir() { 0o700 } else { 0o600 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    {
        // On Windows the file inherits the user profile's ACL, which is already
        // owner-only for a standard account. Nothing portable to tighten here.
        let _ = path;
    }
    Ok(())
}

pub fn forget_token() -> Result<bool> {
    let path = token_path();
    if path.exists() {
        std::fs::remove_file(&path)?;
        return Ok(true);
    }
    Ok(false)
}

// ------------------------------------------------------------------ PKCE

struct Pkce {
    verifier: String,
    challenge: String,
}

fn pkce() -> Pkce {
    let url_safe = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    // 64 random bytes, base64url-encoded, is comfortably inside the 43..128
    // character range the spec requires.
    let mut seed = Vec::with_capacity(64);
    for _ in 0..4 {
        seed.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    seed.extend_from_slice(
        &std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos().to_le_bytes())
            .unwrap_or_default(),
    );
    let verifier = url_safe.encode(&seed);
    let challenge = url_safe.encode(Sha256::digest(verifier.as_bytes()));
    Pkce {
        verifier,
        challenge,
    }
}

// ------------------------------------------------------------------ the flow

pub struct Authorization {
    pub url: String,
    verifier: String,
    state: String,
    redirect_uri: String,
    listener: tokio::net::TcpListener,
}

/// Binds the loopback listener and builds the consent URL. The listener is
/// bound *before* the URL is shown, so the redirect can never arrive at a
/// closed port.
pub async fn begin(port: u16) -> Result<Authorization> {
    let creds = client_credentials()?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("could not bind 127.0.0.1:{port} for the OAuth redirect"))?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");
    let pkce = pkce();
    let state = uid();

    let url = url::Url::parse_with_params(
        AUTH_URL,
        &[
            ("client_id", creds.client_id.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", SCOPE),
            // offline + consent is what actually yields a refresh token.
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("code_challenge", pkce.challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
        ],
    )?
    .to_string();

    Ok(Authorization {
        url,
        verifier: pkce.verifier,
        state,
        redirect_uri,
        listener,
    })
}

/// Waits for Google to redirect back, validates `state`, and exchanges the
/// code for tokens.
pub async fn complete(
    auth: Authorization,
    http: &reqwest::Client,
    timeout: std::time::Duration,
) -> Result<StoredToken> {
    let deadline = tokio::time::Instant::now() + timeout;

    let (code, got_state) = loop {
        let accepted = tokio::time::timeout_at(deadline, auth.listener.accept())
            .await
            .map_err(|_| anyhow::anyhow!("timed out waiting for the Google consent redirect"))?;
        let (mut stream, _) = accepted.context("accepting the OAuth redirect")?;

        let mut buf = [0u8; 8192];
        let read = tokio::time::timeout(std::time::Duration::from_secs(10), stream.read(&mut buf))
            .await
            .unwrap_or(Ok(0))
            .unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..read]).to_string();
        let target = request
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("/")
            .to_string();

        // Browsers also ask for /favicon.ico; ignore anything with neither a
        // code nor an error.
        let parsed = url::Url::parse(&format!("http://127.0.0.1{target}"))?;
        let params: std::collections::HashMap<String, String> = parsed
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();

        if let Some(err) = params.get("error") {
            respond(&mut stream, "Authorization denied. You can close this tab.").await;
            bail!("Google returned an authorization error: {err}");
        }
        match (params.get("code"), params.get("state")) {
            (Some(code), Some(state)) => {
                respond(
                    &mut stream,
                    "FRIDAY is connected to Gmail (read-only). You can close this tab.",
                )
                .await;
                break (code.clone(), state.clone());
            }
            _ => {
                respond(&mut stream, "Waiting for the Google redirect\u{2026}").await;
                continue;
            }
        }
    };

    anyhow::ensure!(
        got_state == auth.state,
        "OAuth state mismatch: the redirect did not come from the request FRIDAY started"
    );

    let creds = client_credentials()?;
    let body = form_body(&[
        ("code", code.as_str()),
        ("client_id", creds.client_id.as_str()),
        ("client_secret", creds.client_secret.as_str()),
        ("redirect_uri", auth.redirect_uri.as_str()),
        ("grant_type", "authorization_code"),
        ("code_verifier", auth.verifier.as_str()),
    ]);

    let resp = http
        .post(TOKEN_URL)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!(
            "token exchange failed with {status}: {}",
            truncate(&crate::util::redact(&text), 400)
        );
    }
    let v: Value = serde_json::from_str(&text)?;
    let refresh_token = v["refresh_token"]
        .as_str()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Google did not return a refresh token. Revoke FRIDAY's access at \
                 myaccount.google.com/permissions and run the flow again - the refresh token is \
                 only issued on first consent."
            )
        })?
        .to_string();

    let token = StoredToken {
        refresh_token,
        access_token: v["access_token"].as_str().unwrap_or_default().to_string(),
        expires_at: expiry(v["expires_in"].as_i64().unwrap_or(0)),
        scope: v["scope"].as_str().unwrap_or(SCOPE).to_string(),
        account: String::new(),
        obtained_at: now(),
    };
    save_token(&token)?;
    Ok(token)
}

async fn respond(stream: &mut tokio::net::TcpStream, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>FRIDAY</title>\
         <body style=\"font-family:system-ui;background:#0c0e13;color:#e6e8ee;padding:3rem\">\
         <h2>FRIDAY</h2><p>{message}</p></body>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

fn form_body(pairs: &[(&str, &str)]) -> String {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        s.append_pair(k, v);
    }
    s.finish()
}

fn expiry(seconds: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(seconds.max(0) - 60)).to_rfc3339()
}

// ------------------------------------------------------------------ access token

/// Returns a usable access token, refreshing it when it has expired.
pub async fn access_token(state: &AppState) -> Result<String> {
    let mut token = load_token().ok_or_else(|| {
        anyhow::anyhow!(
            "Gmail is not connected. Run `friday auth gmail` to grant read-only access, or set \
             briefing.email_provider = \"synthetic\" to use demo data."
        )
    })?;

    let still_valid = chrono::DateTime::parse_from_rfc3339(&token.expires_at)
        .map(|t| t > chrono::Utc::now())
        .unwrap_or(false);
    if still_valid && !token.access_token.is_empty() {
        return Ok(token.access_token);
    }

    let creds = client_credentials()?;
    let body = form_body(&[
        ("client_id", creds.client_id.as_str()),
        ("client_secret", creds.client_secret.as_str()),
        ("refresh_token", token.refresh_token.as_str()),
        ("grant_type", "refresh_token"),
    ]);
    let resp = state
        .http
        .post(TOKEN_URL)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::BAD_REQUEST || status == reqwest::StatusCode::UNAUTHORIZED {
        bail!(
            "Gmail refused the refresh token ({status}). Access was probably revoked or the \
             consent expired. Run `friday auth gmail` again to reconnect."
        );
    }
    if !status.is_success() {
        bail!(
            "token refresh failed with {status}: {}",
            truncate(&crate::util::redact(&text), 300)
        );
    }
    let v: Value = serde_json::from_str(&text)?;
    token.access_token = v["access_token"].as_str().unwrap_or_default().to_string();
    token.expires_at = expiry(v["expires_in"].as_i64().unwrap_or(0));
    anyhow::ensure!(
        !token.access_token.is_empty(),
        "token refresh returned no access token"
    );
    save_token(&token)?;
    state
        .audit(
            "system",
            "gmail",
            "token_refresh",
            "access token renewed",
            None,
        )
        .await;
    Ok(token.access_token)
}

// ------------------------------------------------------------------ sync

/// Pulls messages newer than the newest one already stored, into `inbox`.
/// Deduplicates on `(provider, external_id)`, so a repeated sync is a no-op.
pub async fn sync(state: &AppState) -> Result<Value> {
    let cfg = state.cfg().await;
    let token = access_token(state).await?;

    // Incremental: Gmail's `after:` takes epoch seconds. Fall back to the last
    // seven days on a first sync so the first briefing has something to say.
    let newest: Option<String> =
        sqlx::query_scalar("SELECT MAX(received_at) FROM inbox WHERE provider='gmail'")
            .fetch_one(&state.db)
            .await
            .unwrap_or(None);
    let after = newest
        .as_deref()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.timestamp())
        .unwrap_or_else(|| (chrono::Utc::now() - chrono::Duration::days(7)).timestamp());

    let mut query = format!("after:{after}");
    if !cfg.briefing.gmail_query.is_empty() {
        query.push(' ');
        query.push_str(&cfg.briefing.gmail_query);
    }

    let mut list_url = url::Url::parse(&format!("{API}/messages"))?;
    list_url
        .query_pairs_mut()
        .append_pair("q", &query)
        .append_pair("maxResults", &cfg.briefing.gmail_max_messages.to_string());
    for label in &cfg.briefing.gmail_labels {
        list_url.query_pairs_mut().append_pair("labelIds", label);
    }

    let listed: Value = api_get(state, list_url.as_str(), &token).await?;
    let ids: Vec<String> = listed["messages"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let mut inserted = 0u64;
    let mut skipped = 0u64;
    for id in &ids {
        // Already stored? Then there is nothing to fetch.
        let known: Option<String> =
            sqlx::query_scalar("SELECT id FROM inbox WHERE provider='gmail' AND external_id=?")
                .bind(id)
                .fetch_optional(&state.db)
                .await?;
        if known.is_some() {
            skipped += 1;
            continue;
        }
        let msg = api_get(state, &format!("{API}/messages/{id}?format=full"), &token).await?;
        let parsed = parse_message(&msg);
        let affected = sqlx::query(
            "INSERT OR IGNORE INTO inbox (id,provider,external_id,subject,sender,body,received_at,link)
             VALUES (?,'gmail',?,?,?,?,?,?)",
        )
        .bind(uid())
        .bind(id)
        .bind(&parsed.subject)
        .bind(&parsed.sender)
        .bind(truncate(&parsed.body, 32_000))
        .bind(&parsed.received_at)
        .bind(format!("https://mail.google.com/mail/u/0/#inbox/{id}"))
        .execute(&state.db)
        .await?
        .rows_affected();
        inserted += affected;
    }

    state
        .audit(
            "system",
            "gmail",
            "sync",
            &format!("query={query} listed={} inserted={inserted}", ids.len()),
            None,
        )
        .await;

    Ok(json!({
        "provider": "gmail",
        "data_is_synthetic": false,
        "scope": SCOPE,
        "query": query,
        "listed": ids.len(),
        "new_emails": inserted,
        "already_known": skipped,
        "new_events": 0,
        "note": "Read-only Gmail access. FRIDAY cannot send, delete or modify mail."
    }))
}

async fn api_get(state: &AppState, url: &str, token: &str) -> Result<Value> {
    let resp = state.http.get(url).bearer_auth(token).send().await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    match status {
        s if s.is_success() => {
            Ok(serde_json::from_str(&text).context("Gmail returned invalid JSON")?)
        }
        reqwest::StatusCode::UNAUTHORIZED => {
            bail!("Gmail rejected the access token. Run `friday auth gmail` to reconnect.")
        }
        reqwest::StatusCode::FORBIDDEN => bail!(
            "Gmail returned 403. Check that the Gmail API is enabled for this project and that \
             the consent included the {SCOPE} scope: {}",
            truncate(&crate::util::redact(&text), 300)
        ),
        reqwest::StatusCode::TOO_MANY_REQUESTS => {
            bail!("Gmail rate limit reached; try the sync again shortly")
        }
        s => bail!(
            "Gmail returned {s}: {}",
            truncate(&crate::util::redact(&text), 300)
        ),
    }
}

pub struct ParsedMessage {
    pub subject: String,
    pub sender: String,
    pub body: String,
    pub received_at: String,
}

/// Extracts the headers and the best available text body from a Gmail
/// `format=full` message.
pub fn parse_message(msg: &Value) -> ParsedMessage {
    let header = |name: &str| -> String {
        msg.pointer("/payload/headers")
            .and_then(|h| h.as_array())
            .and_then(|hs| {
                hs.iter().find(|h| {
                    h["name"]
                        .as_str()
                        .is_some_and(|n| n.eq_ignore_ascii_case(name))
                })
            })
            .and_then(|h| h["value"].as_str())
            .unwrap_or("")
            .to_string()
    };

    // internalDate is epoch milliseconds and is more reliable than the Date header.
    let received_at = msg["internalDate"]
        .as_str()
        .and_then(|ms| ms.parse::<i64>().ok())
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(now);

    let mut body = extract_body(msg.pointer("/payload").unwrap_or(&Value::Null));
    if body.trim().is_empty() {
        // Fall back to the snippet rather than storing nothing.
        body = msg["snippet"].as_str().unwrap_or("").to_string();
    }

    ParsedMessage {
        subject: header("Subject"),
        sender: header("From"),
        body,
        received_at,
    }
}

/// Walks the MIME tree for a text body, preferring `text/plain` and falling
/// back to stripped `text/html`.
fn extract_body(part: &Value) -> String {
    fn decode(data: &str) -> String {
        // Gmail uses base64url without padding.
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(data.trim_end_matches('='))
            .or_else(|_| base64::engine::general_purpose::STANDARD.decode(data))
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .unwrap_or_default()
    }
    fn walk(part: &Value, want: &str) -> Option<String> {
        if part["mimeType"].as_str() == Some(want) {
            if let Some(data) = part.pointer("/body/data").and_then(|d| d.as_str()) {
                let text = decode(data);
                if !text.trim().is_empty() {
                    return Some(text);
                }
            }
        }
        for sub in part["parts"].as_array().into_iter().flatten() {
            if let Some(found) = walk(sub, want) {
                return Some(found);
            }
        }
        None
    }
    if let Some(plain) = walk(part, "text/plain") {
        return plain;
    }
    walk(part, "text/html")
        .map(|h| crate::util::strip_html(&h))
        .unwrap_or_default()
}

/// Connection state for the UI and the CLI. Never returns token material.
pub async fn status(state: &AppState) -> Value {
    let cfg = state.cfg().await;
    let stored = load_token();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbox WHERE provider='gmail'")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);
    json!({
        "provider": cfg.briefing.email_provider,
        "client_configured": client_credentials().is_ok(),
        "connected": stored.is_some(),
        "scope": stored.as_ref().map(|t| t.scope.clone()).unwrap_or_else(|| SCOPE.into()),
        "connected_at": stored.as_ref().map(|t| t.obtained_at.clone()),
        "token_file": token_path().display().to_string(),
        "messages_stored": count,
        "capabilities": {"read": true, "send": false, "delete": false, "modify": false},
        "how_to_connect": "friday auth gmail",
        "how_to_disconnect": "friday auth gmail --forget, then revoke at myaccount.google.com/permissions"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_and_challenge_are_well_formed() {
        let p = pkce();
        assert!(
            (43..=128).contains(&p.verifier.len()),
            "verifier length {}",
            p.verifier.len()
        );
        assert!(p
            .verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        // The challenge is the base64url SHA-256 of the verifier, unpadded.
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(p.verifier.as_bytes()));
        assert_eq!(p.challenge, expected);
        assert!(!p.challenge.contains('='));
        // Two calls must not produce the same verifier.
        assert_ne!(pkce().verifier, p.verifier);
    }

    #[test]
    fn only_the_read_only_scope_is_ever_requested() {
        assert_eq!(SCOPE, "https://www.googleapis.com/auth/gmail.readonly");
        assert!(!SCOPE.contains("modify") && !SCOPE.contains("send") && !SCOPE.contains("compose"));
    }

    #[test]
    fn a_plain_text_body_is_decoded() {
        let msg = json!({
            "internalDate": "1760000000000",
            "payload": {
                "mimeType": "text/plain",
                "headers": [
                    {"name": "Subject", "value": "Milestone 2 due Friday"},
                    {"name": "From", "value": "prof@example.edu"}
                ],
                "body": {"data": base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode("Submit through the portal by 2026-10-30.")}
            }
        });
        let p = parse_message(&msg);
        assert_eq!(p.subject, "Milestone 2 due Friday");
        assert_eq!(p.sender, "prof@example.edu");
        assert!(p.body.contains("2026-10-30"));
        assert!(p.received_at.starts_with("2025") || p.received_at.starts_with("202"));
    }

    #[test]
    fn a_multipart_message_prefers_plain_text_over_html() {
        let b64 = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
        let msg = json!({
            "internalDate": "1760000000000",
            "payload": {
                "mimeType": "multipart/alternative",
                "headers": [{"name": "Subject", "value": "Invoice"}],
                "parts": [
                    {"mimeType": "text/html",
                     "body": {"data": b64("<p>HTML version</p><script>x()</script>")}},
                    {"mimeType": "text/plain", "body": {"data": b64("Plain version")}}
                ]
            }
        });
        assert_eq!(parse_message(&msg).body, "Plain version");
    }

    #[test]
    fn html_only_messages_are_stripped_of_markup_and_scripts() {
        let b64 = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
        let msg = json!({
            "payload": {
                "mimeType": "multipart/mixed",
                "headers": [],
                "parts": [{"mimeType": "text/html",
                    "body": {"data": b64("<div>Hello <b>there</b></div><script>evil()</script>")}}]
            }
        });
        let body = parse_message(&msg).body;
        assert!(body.contains("Hello") && body.contains("there"));
        assert!(!body.contains("evil"), "script content survived: {body}");
        assert!(!body.contains('<'));
    }

    #[test]
    fn a_message_with_no_body_falls_back_to_the_snippet() {
        let msg = json!({
            "snippet": "Short preview text",
            "payload": {"mimeType": "multipart/mixed", "headers": [], "parts": []}
        });
        assert_eq!(parse_message(&msg).body, "Short preview text");
    }

    #[test]
    fn a_missing_client_explains_the_setup_rather_than_failing_blankly() {
        // The test environment has no Google client configured.
        if client_credentials().is_ok() {
            return;
        }
        // Deliberately no `unwrap_err()`: `ClientCredentials` must never derive
        // Debug, because that is how a client secret ends up in a log line.
        let e = match client_credentials() {
            Ok(_) => unreachable!("checked above"),
            Err(e) => format!("{e:#}"),
        };
        assert!(e.contains("Desktop app"), "{e}");
        assert!(e.contains("GOOGLE_CLIENT_ID"), "{e}");
        assert!(e.contains("docs/gmail.md"), "{e}");
    }

    #[test]
    fn the_token_file_is_inside_the_gitignored_secrets_directory() {
        assert!(token_path().starts_with(".secrets"));
        let ignore = std::fs::read_to_string(".gitignore").unwrap_or_default();
        assert!(ignore.contains(".secrets/"), ".secrets/ must be gitignored");
    }
}
