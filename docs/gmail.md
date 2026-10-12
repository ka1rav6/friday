# Connecting Gmail

FRIDAY reads your mail with the **read-only** Gmail scope
(`https://www.googleapis.com/auth/gmail.readonly`). With that scope it
*cannot* send, delete, or modify anything in the mailbox — not as a policy
choice that could be changed by a prompt, but because Google will not permit it
with the token FRIDAY holds.

Setup takes about five minutes and only has to be done once.

## What you have to do

### 1. Create a Google Cloud project

1. Go to <https://console.cloud.google.com/projectcreate>.
2. Name it anything (`friday` is fine) and create it.
3. Make sure it is the selected project in the top bar.

### 2. Enable the Gmail API

1. Go to <https://console.cloud.google.com/apis/library/gmail.googleapis.com>.
2. Click **Enable**.

Skipping this step produces a `403` on the first sync, and FRIDAY's error says
so explicitly.

### 3. Configure the consent screen

1. Go to **APIs & Services → OAuth consent screen**.
2. Choose **External**, fill in the app name and your own email, and save.
3. Under **Scopes**, add `.../auth/gmail.readonly` and nothing else.
4. Under **Test users**, add your own Google account.

Leave the app in **Testing**. A testing app can only be used by the test users
you list, which is exactly what you want for a personal assistant — there is no
need to submit anything for verification.

### 4. Create the OAuth client

1. Go to **APIs & Services → Credentials → Create credentials → OAuth client ID**.
2. Application type: **Desktop app**.
3. Create it, then copy the **client ID** and **client secret**.

Desktop app is the right type: FRIDAY receives the authorization code on
`http://127.0.0.1`, so the code never passes through anyone else's server.

### 5. Put the credentials in `.env`

```bash
# in the project root
cp -n .env.example .env
```

Add these two lines:

```
GOOGLE_CLIENT_ID=...apps.googleusercontent.com
GOOGLE_CLIENT_SECRET=...
```

`.env` is gitignored. These never go into `config.toml`, so they cannot leak
through a config export or the API.

### 6. Grant access

```bash
cargo run -- auth gmail
```

FRIDAY prints the scope it is asking for, binds a loopback listener, opens the
consent page, and waits. Approve it in the browser; the tab confirms and you can
close it.

### 7. Switch the briefing over

```toml
# .friday/config.toml
[briefing]
email_provider = "gmail"
gmail_labels = ["INBOX"]
gmail_query = "-category:promotions -category:social"
gmail_max_messages = 25
```

```bash
cargo run -- brief
```

## Verifying it worked

```bash
curl -s localhost:8787/api/accounts/gmail | jq
```

```jsonc
{
  "connected": true,
  "scope": "https://www.googleapis.com/auth/gmail.readonly",
  "messages_stored": 23,
  "capabilities": { "read": true, "send": false, "delete": false, "modify": false }
}
```

No token material is ever returned by this endpoint.

## How the sync works

- **Incremental.** Each sync asks Gmail for `after:<newest message already
  stored>`, falling back to the last seven days on a first run. Messages are
  deduplicated on `(provider, external_id)`, so running it repeatedly inserts
  nothing new.
- **Bounded.** At most `gmail_max_messages` per sync, bodies truncated, and the
  message list filtered by `gmail_labels` and `gmail_query` before anything is
  fetched.
- **Body extraction** walks the MIME tree preferring `text/plain`, falls back to
  `text/html` with markup and scripts stripped, and finally to Gmail's own
  snippet — so a message never lands in the database empty.
- **Links back.** Every stored message keeps a
  `https://mail.google.com/mail/u/0/#inbox/<id>` link, so a briefing item can
  always be traced to the original.

## Security

| Concern | What FRIDAY does |
| --- | --- |
| Scope | `gmail.readonly` only. Asserted by a unit test. |
| Client secret | Environment only; `ClientCredentials` deliberately does not derive `Debug`, so it cannot be printed by accident. |
| Authorization code | Returned to `127.0.0.1` on a listener bound *before* the URL is shown. |
| Code interception | PKCE (S256). The `state` parameter is checked, so a redirect from a request FRIDAY did not start is rejected. |
| Token storage | `.secrets/google.json`, mode `0600`, directory `0700`, gitignored. An OS keyring would be better; this is the documented fallback. |
| Token refresh | Automatic. A revoked grant produces "run `friday auth gmail` again", not an obscure 400. |
| Prompt injection | Message bodies are untrusted input. They reach the model only inside `<untrusted_content>`, and the briefing scans for override phrasings and reports them as `injection_warnings`. |
| Audit | Connect, refresh, sync and disconnect are all written to the audit log. |

The OAuth flow is **CLI-only on purpose**. Exposing it over HTTP would let a
web page trigger a consent prompt; `POST` to an auth endpoint does not exist.

## Disconnecting

```bash
cargo run -- auth gmail --forget     # deletes the local token
```

Then revoke the grant at <https://myaccount.google.com/permissions> so it is
invalid on Google's side too. FRIDAY's output says this as well, because
deleting a local file is not revocation.

## Troubleshooting

| Symptom | Cause |
| --- | --- |
| "Gmail is not configured" | `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET` missing from `.env`. |
| "Google did not return a refresh token" | You had already consented. Revoke at myaccount.google.com/permissions and retry — the refresh token is only issued on first consent. |
| `403` with a scope message | The Gmail API is not enabled, or consent did not include the read-only scope. |
| "could not bind 127.0.0.1:8788" | Something else holds the port. Change `briefing.oauth_port`. |
| "access blocked: this app is not verified" | Add your account under **Test users** on the consent screen. |
| Sync returns 0 new messages | Expected on a second run — it is incremental. Widen `gmail_query` or check `gmail_labels`. |

## Microsoft Graph

Not implemented. Selecting `email_provider = "microsoft"` returns an error
saying so, rather than silently serving demo data. The connector would slot into
`briefing::sync` the same way Gmail does.
