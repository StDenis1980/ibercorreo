# Tinta Correo — testing

Status: 2026-10-26 · Unit, scripted-server and local-TLS tests run in `cargo xtask ci`; real
IMAP, SMTP, CalDAV and CardDAV servers are tested when configured.

## Layers of tests
- **Unit and property tests** in each crate (`cargo test -p <crate>`), including hostile-input
  tests that must never panic (`hostile_servers_never_panic`, `every_command_survives_hostile_params`).
- **Scripted server** (`crates/imap/src/tests.rs`): the IMAP client against replies written in
  advance, checked byte for byte.
- **Local TLS** (`crates/net/src/tests.rs`): real TCP and rustls on 127.0.0.1 against a tiny server
  with a generated certificate: implicit TLS, STARTTLS, untrusted and mismatched certificates,
  passwords refused over plain connections.
- **Sync logic** (`crates/sync/src/tests.rs`): `state`/`apply` on stores, `run` against a scripted server.
  Calendar sync (`crates/sync/src/calendar/tests.rs`) and contact sync
  (`crates/sync/src/contacts/tests.rs`) run against small CalDAV and CardDAV servers in memory.
- **HTTP, WebDAV and CalDAV** (`crates/dav/src/tests.rs`): replies read two bytes at a time,
  chunked and keep-alive replies, hostile replies, XML bounds (depth, no `DOCTYPE`), discovery.
- **Real server** (`real_server_when_configured`, `real_server_sync_when_configured`,
  `real_submission_when_configured`): skipped unless
  `IBERCORREO_TEST_IMAP` (and for submission `IBERCORREO_TEST_SMTP`) is set. The sync test reads, flags, moves and deletes here and checks the
  server, then changes the server and checks the store.

## A throwaway Dovecot for the real-server test (Linux)
Dovecot runs only as an external test server; none of its code is used.

```sh
sudo apt-get install dovecot-imapd
D=/tmp/iber-dovecot; mkdir -p $D/mail $D/run; cd $D
openssl req -x509 -newkey rsa:2048 -nodes -keyout key.pem -out cert.pem -days 30 -subj /CN=localhost \
  -addext subjectAltName=DNS:localhost -addext basicConstraints=critical,CA:FALSE \
  -addext extendedKeyUsage=serverAuth
openssl x509 -in cert.pem -outform der -out cert.der
sudo chown -R dovecot:dovecot $D/mail
```

`$D/dovecot.conf` (replace `$D` with the path): any user name, password `secret`.

```
protocols = imap
listen = 127.0.0.1
base_dir = $D/run
state_dir = $D/run
log_path = $D/dovecot.log
ssl = yes
ssl_cert = <$D/cert.pem
ssl_key = <$D/key.pem
disable_plaintext_auth = yes
mail_location = maildir:$D/mail/%u
passdb {
  driver = static
  args = password=secret
}
userdb {
  driver = static
  args = uid=dovecot gid=dovecot home=$D/mail/%u
}
service imap-login {
  inet_listener imap {
    port = 10143
  }
  inet_listener imaps {
    port = 10993
    ssl = yes
  }
}
namespace inbox {
  inbox = yes
  mailbox Sent {
    special_use = \Sent
    auto = create
  }
  mailbox Trash {
    special_use = \Trash
    auto = create
  }
}
default_internal_user = dovecot
default_login_user = dovenull
```

```sh
sudo dovecot -c $D/dovecot.conf
IBERCORREO_TEST_IMAP="localhost 10993 10143 denys secret $D/cert.der" cargo test -p ibercorreo-net real_
```

Several accounts: with Dovecot taking any user name, two accounts on the same server are two
mailboxes. Deliver 3 messages to `ana` and 520 to `bob` (`first_valid_uid = 1` in the config lets
`doveadm save -u bob` write as the `dovecot` user), then:

```sh
C=ibercorreo-cli; S=/tmp/two.json
$C run --store $S --save --cmd 'account.set={"email":"ana@example.org","user":"ana","imapHost":"localhost","imapPort":10993,"smtpHost":"localhost"}' \
  --cmd 'account.add={"email":"bob@example.org","user":"bob","imapHost":"localhost","imapPort":10993,"smtpHost":"localhost"}'
IBERCORREO_PASSWORD=secret $C sync --store $S --ca $D/cert.der --account 0   # added 3
IBERCORREO_PASSWORD=secret $C sync --store $S --ca $D/cert.der --account 1   # added 500, remaining 20
IBERCORREO_PASSWORD=secret $C sync --store $S --ca $D/cert.der --account 1   # added 20, remaining 0
$C run --store $S --cmd folder.list   # each Inbox in its own account
```

Changing account 1's sign-in name (`account.set` with `"id":1`) empties its folders. In memory,
`crates/store`, `crates/sync`, `crates/engine` and `crates/ui-egui` test the same (folders per
account, old stores, forgetting, sending from each account's own folders, the servers filled in).

`real_idle_when_configured` (in the same run) leaves an `IDLE` session on the Inbox, appends a
message from a second connection and expects the watch to wake within 15 seconds, then checks
that stopping ends it.

Autoconfiguration has its own checks: `crates/dav/src/autoconfig.rs` reads invented files and,
with `IBERCORREO_TEST_ISPDB_DIR=dir` (files named `<domain>.xml`, for example downloaded from
the public `thunderbird/autoconfig` repository's `ispdb/` folder, never committed), real ones;
`crates/net/src/autoconfig.rs` tests the search order, redirects and fallbacks with a fake
network, and `autoconfig_probes_and_fetches_over_real_tls` probes servers over local TLS.

### OAuth 2.0 (XOAUTH2)
The flow is tested without a provider: `crates/net/src/oauth.rs` checks PKCE against RFC 7636's
example, the authorization address, token answers (good, refused, revoked, malformed), the
loopback listener (a stray request and another sign-in's redirect are waited past, a denied
consent cancels) and token renewal and rotation; `oauth_token_requests_over_real_tls` posts a
token request over local TLS; the IMAP and SMTP clients have scripted XOAUTH2 tests.

End to end, by hand: a small test provider on this machine (`/authorize` answering with a redirect
and a code, `/token` over HTTPS checking the PKCE verifier and rotating refresh tokens, and
`/introspect` over plain HTTP for Dovecot) and Dovecot taking XOAUTH2 through its `oauth2` passdb:

```
auth_mechanisms = plain login xoauth2 oauthbearer
passdb {
  driver = oauth2
  mechanisms = xoauth2 oauthbearer
  args = /tmp/ibercorreo-dovecot/dovecot-oauth2.conf.ext   # introspection_mode = post,
}                                                          # introspection_url = http://127.0.0.1:PORT/introspect,
passdb {                                                   # username_attribute = username,
  driver = static                                          # active_attribute = active, active_value = true
  mechanisms = plain login
  args = password=secret
}
```

Point Tinta Correo at it with `IBERCORREO_GOOGLE_CLIENT_ID`, `IBERCORREO_GOOGLE_AUTH_URL`,
`IBERCORREO_GOOGLE_TOKEN_URL` (and `IBERCORREO_CA`), and set `BROWSER` to a script that follows
the sign-in address with `curl -L` (it stands in for the person consenting). Then
`ibercorreo-cli oauth google ana@example.org` prints a refresh token, `ibercorreo-cli sync` with
it signs in to Dovecot (`method=XOAUTH2` in its log) and sends through Postfix (whose SASL is
Dovecot's), and in the window **Save and sign in** does the same, the Inbox watch included.

## Postfix for the submission test (Linux)
Postfix also runs only as an external test server. It takes mail on 10587 (STARTTLS) and 10465
(implicit TLS), checks passwords through Dovecot, and delivers to Dovecot over LMTP, so a sent
message can be found in the recipient's INBOX.

```sh
sudo apt-get install postfix dovecot-lmtpd    # "Local only"
sudo cp $D/cert.pem $D/key.pem /etc/postfix/
sudo postconf -e "inet_interfaces = loopback-only" "mydestination = localhost" \
  "smtpd_tls_cert_file = /etc/postfix/cert.pem" "smtpd_tls_key_file = /etc/postfix/key.pem" \
  "smtpd_sasl_type = dovecot" "smtpd_sasl_path = private/auth" \
  "virtual_transport = lmtp:unix:private/dovecot-lmtp" "virtual_mailbox_domains = example.org" \
  "smtpd_relay_restrictions = permit_sasl_authenticated,reject"
```

Append to `/etc/postfix/master.cf`:

```
10587     inet  n       -       y       -       -       smtpd
  -o smtpd_tls_security_level=encrypt
  -o smtpd_sasl_auth_enable=yes
  -o smtpd_tls_auth_only=yes
10465     inet  n       -       y       -       -       smtpd
  -o smtpd_tls_wrappermode=yes
  -o smtpd_sasl_auth_enable=yes
```

Add to `$D/dovecot.conf` (and `lmtp` to `protocols`):

```
service auth {
  unix_listener /var/spool/postfix/private/auth {
    mode = 0660
    user = postfix
    group = postfix
  }
}
service lmtp {
  unix_listener /var/spool/postfix/private/dovecot-lmtp {
    mode = 0600
    user = postfix
    group = postfix
  }
}
```

```sh
sudo postfix start
IBERCORREO_TEST_SMTP="localhost 10587 starttls" \
IBERCORREO_TEST_IMAP="localhost 10993 10143 denys secret $D/cert.der" cargo test -p ibercorreo-net real_
```

## Radicale for the CalDAV test (Linux)
Radicale (GPL-3.0) runs only as an external test server; none of its code is used.

```sh
python3 -m venv $D/rad && $D/rad/bin/pip install radicale
mkdir -p $D/radicale && echo 'ana:secret' > $D/radicale/users
cat > $D/radicale/config <<CONF
[server]
hosts = 127.0.0.1:15232
ssl = True
certificate = $D/cert.pem
key = $D/key.pem
[auth]
type = htpasswd
htpasswd_filename = $D/radicale/users
htpasswd_encryption = plain
[storage]
filesystem_folder = $D/radicale/collections
CONF
$D/rad/bin/radicale --config $D/radicale/config &
curl -sk -u ana:secret -X MKCALENDAR https://127.0.0.1:15232/ana/work/
IBERCORREO_TEST_CALDAV="https://localhost:15232/ana/work/ ana secret $D/cert.der" cargo test -p ibercorreo-net real_caldav
```

The test fills and empties `/ana/work/`, and checks that changes made in the store (an edit, a new
appointment, a deletion) reach the server. To try the app: `caldav` `https://localhost:15232/`
in Account settings (discovery goes through `/.well-known/caldav`) and `IBERCORREO_CA=$D/cert.der`.

For CardDAV, make an address book on the same Radicale and run the contact test:

```sh
curl -sk -u ana:secret -X MKCOL https://127.0.0.1:15232/ana/contacts/ --data '<?xml version="1.0"?>
<d:mkcol xmlns:d="DAV:" xmlns:r="urn:ietf:params:xml:ns:carddav"><d:set><d:prop><d:resourcetype>
<d:collection/><r:addressbook/></d:resourcetype></d:prop></d:set></d:mkcol>'
IBERCORREO_TEST_CARDDAV="https://localhost:15232/ana/contacts/ ana secret $D/cert.der" cargo test -p ibercorreo-net real_carddav
```

It fills and empties `/ana/contacts/`, sends an edit, a new contact and a deletion, and checks a
fresh store sees them. In the app the same `https://localhost:15232/` address finds the address
books through `/.well-known/carddav`.

## Meeting invitations end to end
With Postfix, Dovecot and Radicale as above (and a Radicale user named like the mail sign-in,
since the calendar uses the same name and password): send an invitation to the test mailbox
(a `multipart/alternative` message with a `text/calendar; method=REQUEST` part, for example
with Python's `smtplib`), then

```sh
ibercorreo-cli sync --store s.json --ca $D/cert.der           # the invitation arrives
ibercorreo-cli run --store s.json --save --cmd 'invite.respond={"id":1,"response":"accept","calendar":1}'
ibercorreo-cli sync --store s.json --ca $D/cert.der           # answer sent, meeting PUT
```

The organizer's mailbox gets the `METHOD:REPLY` with `PARTSTAT=ACCEPTED`, and the calendar
collection holds the meeting. In memory, `crates/engine/src/tests.rs` covers accepting, updating,
declining, editing the answer and cancelling one day of a series.

## Revision history
- 2026-10-22 — created with M1.2 (local TLS and the Dovecot check).
- 2026-10-22 — M1.3: sync tests.
- 2026-10-22 — M1.4: SMTP tests and the Postfix setup.
- 2026-10-23 — M2.5a: HTTP/WebDAV/CalDAV tests, calendar sync tests and the Radicale setup.
- 2026-10-23 — M2.5b: push tests (in memory and against Radicale).
- 2026-10-23 — M2.6: invitations end to end.
- 2026-10-23 — M2.7b: contact sync tests and the CardDAV check.
- 2026-10-23 — M3.1: IDLE and autoconfiguration tests.
- 2026-10-23 — M3.2: OAuth tests and the XOAUTH2 check against Dovecot and Postfix.
- 2026-10-26 — M3.3: the two-account check against Dovecot.
