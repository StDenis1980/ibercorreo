# Tinta Correo on the GNOME desktop

Status: 2026-11-04 · the desktop assistant over MCP added (Vilna OS requests tintaoffice-web#16, #28)

Four things tie Correo into a GNOME desktop. The first three are Linux (and BSD) only, need nothing on
other systems, and change nothing when the desktop does not run the GNOME services they talk to; the
fourth, the MCP server, works on every system.

| What | Where in the code | Needs |
|---|---|---|
| Opening `.ics`, `.vcf`, `.eml` and `mailto:` links, one window per mailbox | `apps/ibercorreo/src/main.rs` (`open_requested`), `single_instance.rs`, `crates/engine/src/mailto.rs` | the desktop file (`packaging/linux/`) |
| Correo's events in the clock panel (GNOME Shell's calendar) | `apps/ibercorreo/src/gnome_calendar.rs` | Evolution Data Server (EDS), which GNOME Shell itself depends on |
| Accounts from GNOME Settings (GNOME Online Accounts) | `apps/ibercorreo/src/goa.rs`, `Account::goa` and `Account::dav_goa` | `goa-daemon` |
| The desktop assistant reads and drafts mail, events and contacts (MCP), in the open window when there is one | `apps/ibercorreo-cli/src/mcp.rs`, `crates/engine/src/instance.rs`, `docs/mcp.md` | an MCP client starting `ibercorreo-cli mcp` |

## Events in the clock panel

The panel shows the selected calendars of EDS. Correo writes its events to
`~/.local/share/tinta-correo/calendar.ics` (mode 0600, through a temporary file and a rename) and,
the first time, registers that file as an EDS calendar called "Tinta Correo": a `local` source whose
`[Local Backend] CustomFile` is the file's `file://` URI. While the source registry runs, the
source is created through its D-Bus API (`SourceManager.CreateSources` on
`org.gnome.evolution.dataserver.Sources5`), which shows it at once; otherwise the key file
`~/.config/evolution/sources/tinta-correo.source` is written and read at the next login. EDS
watches the file, so a change in Correo shows in the panel within a few seconds. Correo never
reads the file back: editing stays in Correo.

Nothing happens where `~/.config/evolution/sources` does not exist (no EDS), nor with `--sample`.

**No event twice.** Calendars that already reach EDS another way are left out of the file: the
CalDAV calendars of an account that signs in to them through GNOME Online Accounts (below), which
EDS mirrors from the same server itself. For those, a change made in Correo is sent to the server
at once (a Send/Receive within seconds, at most every 15 s), and after that Send/Receive Correo
asks EDS to fetch that account's calendars again (`CalendarFactory.OpenCalendar` on
`org.gnome.evolution.dataserver.Calendar8`, then `Open`, `Refresh` and `Close` on each calendar of
the account's collection source), so the panel shows the change without waiting for EDS's own
refresh. What fails there is left: EDS refreshes on its own later.

Flatpak: `--filesystem=xdg-config/evolution/sources:create`, `--filesystem=xdg-data/tinta-correo:create`
and `--talk-name=org.gnome.evolution.dataserver.Sources5` (plus `…Calendar8` for the refresh). Inside
the sandbox `XDG_DATA_HOME` and `XDG_CONFIG_HOME` point into it, so Correo uses the host's
`~/.local/share` and `~/.config` there.

## Accounts from GNOME Settings

Account settings shows **From GNOME Settings** when GNOME Online Accounts answers on the session
bus, with the accounts a person added under Online Accounts. Choosing one fills the form:

- An account with mail (Google, IMAP and SMTP, Microsoft 365 where GNOME gives mail): the address,
  the name, the servers and the sign-in name, and the calendars address if no other account syncs
  calendars. The account remembers GNOME's id (`goa`); no password field, no browser sign-in.
- An account with only calendars and contacts (Nextcloud, WebDAV): its CalDAV/CardDAV address goes
  to the account open in the form, which signs in to it as that GNOME account, with its own name
  (`davGoa`). The mail is untouched.

Correo never copies the secret. At each sign-in it asks GNOME (`OAuth2Based.GetAccessToken` for an
OAuth account, `PasswordBased.GetPassword` otherwise: `imap-password` and `smtp-password` for an
IMAP and SMTP account, `password` for Nextcloud and WebDAV), so a password changed or an account
signed in again in Settings is used at once, and removing the account there stops Correo's access.
When GNOME refuses (an expired sign-in, an account removed), Account settings opens and says so.
**Unlink** goes back to a password or a browser sign-in.

What GNOME gives, checked against goa-daemon 3.50 (GNOME 46):

| Provider | Mail | Calendars and contacts | Secret |
|---|---|---|---|
| Google | imap.gmail.com, smtp.gmail.com | CalDAV `apidata.googleusercontent.com/caldav/v2/…`, CardDAV `www.googleapis.com/.well-known/carddav` | OAuth 2.0 token (GNOME's own Google client: no "unverified app" page, no 7-day re-sign-in) |
| IMAP and SMTP | the servers typed in Settings | none | two passwords |
| Nextcloud | none | `https://<server>/remote.php/dav` | password |
| WebDAV | none | the CalDAV and CardDAV addresses typed in Settings | password |
| Microsoft 365 (`ms_graph`) | none in GNOME 46 (files only) | none in GNOME 46 | OAuth 2.0 token |

Left out on purpose: IMAP or SMTP without encryption, calendar addresses that are not `https://`,
and accounts with nothing Correo uses. Correo finds address books from the calendars address, so
for Google the contacts come only if the CalDAV address serves them too; to be checked on a real
Google account. Microsoft 365 needs checking on GNOME 49: whatever of `Mail`, `Calendar` and
`Contacts` it gives is taken the same way.

Flatpak: `--talk-name=org.gnome.OnlineAccounts`.

`ibercorreo-cli sync` does not ask GNOME: for an account linked to GNOME Settings it takes the
password from `IBERCORREO_PASSWORD`, or says to send and receive in the window.

## Testing

Unit tests cover what Correo reads from GNOME (`goa::tests`, with the objects goa-daemon 3.50
lists), the form (`an_account_from_gnome_settings_fills_the_form_and_saves_without_a_password`),
the command parameters (`an_account_can_sign_in_through_gnome_settings`), the calendar file and
the EDS lookups (`gnome_calendar::tests`).

`goa::tests::a_running_goa_daemon_answers` runs against a real goa-daemon when `TINTA_GOA_LIVE` is
set. The setup, in a throwaway home on a machine with gnome-online-accounts and gnome-keyring:

1. `~/.config/goa-1.0/accounts.conf` with three synthetic accounts:
   `[Account account_1700000000_0]` `Provider=owncloud`, `Identity=ana`,
   `PresentationIdentity=ana@cloud.example.org`, `Uri=https://cloud.example.org`,
   `CalendarEnabled=true`, `ContactsEnabled=true`;
   `[Account account_1700000000_1]` `Provider=imap_smtp`, `EmailAddress=ana@example.org`,
   `ImapHost=imap.example.org`, `ImapUserName=ana`, `ImapUseSsl=true`, `SmtpHost=smtp.example.org`,
   `SmtpUserName=ana`, `SmtpUseAuth=true`, `SmtpUseTls=true`, `MailEnabled=true`;
   `[Account account_1700000000_2]` `Provider=google`, `Identity=ana@gmail.com`, `MailEnabled=true`,
   `CalendarEnabled=true`.
2. The secrets in the keyring under the schema `org.gnome.OnlineAccounts`, attribute
   `goa-identity` = `<provider>:gen<N>:<id>`, as GVariant text: `{'password': <'nc-secret'>}` for
   Nextcloud, `{'imap-password': <'imap-secret'>, 'smtp-password': <'smtp-secret'>}` for IMAP and
   SMTP (writing every generation from 0 to 29 is the simplest way to hit the provider's).
3. `dbus-run-session`, unlock the keyring (`echo -n x | gnome-keyring-daemon --unlock
   --components=secrets`), start `/usr/libexec/goa-daemon --replace`, then run the test binary with
   `TINTA_GOA_LIVE=1`.

The same session with `evolution-source-registry` running and the app started on a store linked to
those accounts shows the whole path: GNOME's refusal of the synthetic Google token opens Account
settings with GNOME's reason, and the form names both linked accounts.

## Revision history

- 2026-11-03: first version: the clock panel, accounts from GNOME Settings.
- 2026-11-04: the assistant over MCP (`docs/mcp.md`).
