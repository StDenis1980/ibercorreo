<p align="center"><img src="assets/app-icon/ibercorreo-1024.png" width="128" alt="Iber Correo icon"></p>

# Iber Correo

**Пошта, календар і контакти для IberOffice** — вільна програма на чистому Rust, у тому ж стилі,
що й Iber PDF, Iber Texto, Iber Calculo та Iber Presenta. Працює на macOS, Windows, Linux і BSD.

*Mail, calendar and people for the IberOffice suite — an open-source, clean-room, Outlook-style
client in pure Rust.*

> **Стан: M0, каркас.** Уже є: модель пошти й календаря, читання й запис листів (RFC 5322/MIME),
> імпорт і експорт календарів `.ics`, локальна скринька з відміною дій, інтерфейс у стилі Outlook
> (пошта, календар на місяць, люди) і командний рядок. **Ще немає мережі:** підключення
> IMAP/SMTP — це M1, CalDAV/CardDAV — M2, Exchange — M4. Див. [дорожню карту](docs/roadmap.md).

## Запуск

```sh
cargo run --release -p ibercorreo -- --sample      # демо-скринька
cargo run --release -p ibercorreo                  # ваша скринька (файл у папці даних системи)
cargo run --release -p ibercorreo -- лист.eml зустріч.ics   # імпорт файлів
```

Командний рядок (усе, що можна клацнути, можна зробити скриптом):

```sh
cargo run -p ibercorreo-cli -- commands
cargo run -p ibercorreo-cli -- run --sample --cmd message.list --cmd 'event.list={"date":"today"}'
cargo run -p ibercorreo-cli -- ics зустріч.ics
```

## Архітектура

| Шар | Крейт | Що робить |
|---|---|---|
| L0 | `core` | модель: облікові записи, папки, листи, адреси, календарі, події, контакти, дати, часові пояси (база IANA через `jiff`, вбудована) |
| L1 | `mime`, `ical` | листи RFC 5322/MIME (читання через `mail-parser`, запис свій); iCalendar RFC 5545 |
| L2 | `store` | локальне сховище й файл, атомарне збереження |
| L4 | `engine` | сесія, відміна дій, реєстр команд, демо-дані |
| L6 | `ui-egui` | інтерфейс у стилі Outlook (замінний) |
| L7 | `apps/ibercorreo`, `apps/ibercorreo-cli` | програми |

Правила для внеску: [AGENTS.md](AGENTS.md). Перевірка перед комітом: `cargo xtask ci`.
Інструкція користувача: [docs/user-guide.md](docs/user-guide.md).

## Ліцензія

MIT або Apache-2.0 на ваш вибір ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).
Усі іконки — оригінальна робота IberOffice ([ATTRIBUTION.md](ATTRIBUTION.md)).
