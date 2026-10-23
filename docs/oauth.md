# Tinta Correo — OAuth 2.0 sign-in (Gmail, Microsoft 365)

Status: 2026-10-23 · M3.2 · the flow works; a build needs client IDs registered by the project

Gmail, Outlook.com and Microsoft 365 do not take a mail program's password: the person signs in
on the provider's own page and the program gets a token. Tinta Correo does this the way RFC 8252
describes for native apps:

1. Account settings → **Sign in with: Google / Microsoft** → **Save and sign in**.
2. Tinta Correo opens a one-time listener on `127.0.0.1` (a free port) and opens the provider's
   sign-in page in the browser, with a PKCE challenge (S256) and a random `state`.
3. The person signs in there and allows access to their mail. The browser comes back to the
   listener with a code; Tinta Correo exchanges the code and the PKCE verifier for an access
   token (about an hour) and a refresh token.
4. The refresh token goes into the system credential store (service `Iber Correo OAuth`, the app's first name, account
   `google:<login>` or `microsoft:<login>`). Access tokens stay in memory and are renewed before
   they run out. IMAP and SMTP sign in with `XOAUTH2`.

The password never reaches Tinta Correo. A refresh token the provider replaces (Microsoft does on
renewals) is stored again. A revoked grant (the person removed the app's access, or changed their
password) is deleted and Account settings asks to sign in again. Access can be removed at any
time at <https://myaccount.google.com/permissions> or <https://account.live.com/consent/Manage>
(work accounts: <https://myapps.microsoft.com>).

Code: `crates/net/src/oauth.rs` (the flow), `AUTHENTICATE XOAUTH2` in `crates/imap`, `AUTH
XOAUTH2` in `crates/smtp`, the window part in `apps/ibercorreo/src/main.rs`.

## Client IDs: registering Tinta Correo with the providers
Each provider wants the program registered with it once. The registration belongs to whoever
publishes the builds (for Tinta Office: the project owner), not to each user. A build without a
client ID shows the provider's choice greyed out.

### Google (Gmail)
1. Open <https://console.cloud.google.com/>, create a project (for example `Tinta Office`).
2. **APIs & Services → Library**: enable **Gmail API**.
3. **Google Auth Platform → Branding**: the app name (`Tinta Correo`), a support email, the logo if
   wanted. **Audience**: *External*, publishing status *Testing*; under **Test users** add the
   Gmail addresses that will use it (up to 100).
4. **Data access → Add or remove scopes**: add `https://mail.google.com/` (full IMAP and SMTP
   access; Google counts it as *restricted*).
5. **Clients → Create client**: application type **Desktop app**. Copy the **Client ID** and the
   **Client secret**. For desktop apps Google's "secret" is not secret (RFC 8252 §8.5): it ships
   inside the program.

While the app is in *Testing*, only the listed test users can sign in and Google ends the grant
after 7 days (sign in again weekly). Opening it to everyone needs Google's verification; for a
restricted scope that includes a yearly third-party security assessment (CASA), which costs money
and time. Until then, other Gmail users can sign in with an **app password** (Google account →
Security → 2-Step Verification → App passwords) and **Sign in with: Password**.

### Microsoft (Outlook.com, Microsoft 365)
1. Open <https://entra.microsoft.com/> (or the Azure portal) → **App registrations → New
   registration**.
2. Name `Tinta Correo`; **Supported account types**: *Accounts in any organizational directory and
   personal Microsoft accounts*; **Redirect URI**: platform *Public client/native (mobile &
   desktop)*, `http://localhost` (any port is accepted for it).
3. **API permissions → Add a permission → Microsoft Graph → Delegated**: `IMAP.AccessAsUser.All`,
   `SMTP.Send`, `offline_access`.
4. Copy the **Application (client) ID**. No secret is needed.

Personal accounts work at once. Work and school accounts may need their administrator's consent
for the app, and Authenticated SMTP must be on for the mailbox (Microsoft 365 admin center →
the user → Mail → Manage email apps). Until the publisher is verified (a Microsoft Partner
Network ID), the consent page says the publisher is unverified, and some organisations block
such apps.

## Putting the client IDs into a build
They are read at compile time, and the same variables at run time win (for trying another
registration without rebuilding):

| Variable | Value |
|---|---|
| `IBERCORREO_GOOGLE_CLIENT_ID` | Google's Client ID (`….apps.googleusercontent.com`) |
| `IBERCORREO_GOOGLE_CLIENT_SECRET` | Google's Client secret |
| `IBERCORREO_MICROSOFT_CLIENT_ID` | Microsoft's Application (client) ID |

```powershell
# Windows (PowerShell), before building
$env:IBERCORREO_GOOGLE_CLIENT_ID = "…apps.googleusercontent.com"
$env:IBERCORREO_GOOGLE_CLIENT_SECRET = "GOCSPX-…"
$env:IBERCORREO_MICROSOFT_CLIENT_ID = "00000000-0000-0000-0000-000000000000"
cargo build --release -p ibercorreo -p ibercorreo-cli
```

```sh
# macOS, Linux
IBERCORREO_GOOGLE_CLIENT_ID=… IBERCORREO_GOOGLE_CLIENT_SECRET=… IBERCORREO_MICROSOFT_CLIENT_ID=… \
  cargo build --release -p ibercorreo -p ibercorreo-cli
```

Keep them out of the repository (forks would otherwise sign in as Tinta Office): in CI, use GitHub
Actions variables or secrets passed as these environment variables.

## Testing against a test provider
`IBERCORREO_GOOGLE_AUTH_URL` and `IBERCORREO_GOOGLE_TOKEN_URL` (and the `MICROSOFT` ones) point
the flow at another server; the token address must be `https://`. `BROWSER` names the program
that opens the sign-in page. `docs/testing.md` (OAuth 2.0) describes the end-to-end check with
Dovecot and Postfix.

## Not yet
- OAuth for calendars and contacts: Google's CalDAV and CardDAV need their own scopes, so a Gmail
  account syncs mail only (Microsoft has no CalDAV).
- Other OAuth providers (Yahoo, AOL, Fastmail, Yandex) and `OAUTHBEARER` (RFC 7628).
- Microsoft 365 may file sent mail in Sent Items itself, giving two copies there (Gmail is known
  and gets none).

## Revision history
- 2026-10-23 — created (M3.2).
