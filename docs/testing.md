# Iber Correo — testing

Status: 2026-10-22 · Unit, scripted-server and local-TLS tests run in `cargo xtask ci`; a real
IMAP server is tested when one is configured.

## Layers of tests
- **Unit and property tests** in each crate (`cargo test -p <crate>`), including hostile-input
  tests that must never panic (`hostile_servers_never_panic`, `every_command_survives_hostile_params`).
- **Scripted server** (`crates/imap/src/tests.rs`): the IMAP client against replies written in
  advance, checked byte for byte.
- **Local TLS** (`crates/net/src/tests.rs`): real TCP and rustls on 127.0.0.1 against a tiny server
  with a generated certificate: implicit TLS, STARTTLS, untrusted and mismatched certificates,
  passwords refused over plain connections.
- **Sync logic** (`crates/sync/src/tests.rs`): `state`/`apply` on stores, `run` against a scripted server.
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
IBERCORREO_TEST_IMAP="localhost 10993 10143 denys secret $D/cert.der" cargo test -p ibercorreo-net real_server
```

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

## Revision history
- 2026-10-22 — created with M1.2 (local TLS and the Dovecot check).
- 2026-10-22 — M1.3: sync tests.
- 2026-10-22 — M1.4: SMTP tests and the Postfix setup.
