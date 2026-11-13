# Перехід з Outlook на Tinta Correo · De Outlook a Tinta Correo · Moving from Outlook

Status: 2026-11-13 · Tinta Correo 0.2 (Preview 4 in progress): `.msg`, `.pst` (mail, calendars,
contacts) and export to mbox / iCalendar / vCard.

- [Українською](#українською)
- [En español](#en-español)
- [In English](#in-english)

## Українською

### 1. Що переносити
- **Пошта на сервері** (Gmail, Outlook.com, Microsoft 365, Exchange, будь-яка IMAP-пошта) нікуди
  переносити не треба. Додайте той самий обліковий запис у Tinta Correo (див. «Підключення пошти» в
  [інструкції](user-guide.md)), і листи, папки, календар та контакти з'являться самі.
- **Файл `.pst`** потрібен для того, що лежить лише на вашому комп'ютері: архіви, папки «На моєму
  комп'ютері», старі облікові записи POP3, локальний календар і контакти.
- **Окремі листи `.msg`** відкриваються в Correo по одному.

### 2. Експорт з Outlook у `.pst`
У класичному Outlook для Windows: **Файл → Відкрити й експортувати → Імпорт і експорт → Експорт у
файл → Файл даних Outlook (.pst)**. Виберіть верхній рівень облікового запису (вашу адресу),
поставте позначку **Включно з вкладеними папками** і збережіть файл, наприклад, на робочий стіл.
- Новий Outlook для Windows і Outlook для macOS файлів `.pst` не створюють. Якщо ваша пошта на
  сервері, просто додайте обліковий запис (крок 1). Якщо ні, експортуйте в класичному Outlook.
- Файл `.ost` (кеш облікового запису Exchange або Microsoft 365) Correo не читає: зробіть з нього
  `.pst` так, як описано вище.

### 3. Імпорт у Tinta Correo
Клацніть файл `.pst` правою кнопкою → **Відкрити за допомогою** → **Tinta Correo**, або
перетягніть його у вікно програми. Великий архів читається у фоні, а в рядку стану видно,
скільки вже прочитано.
- **Пошта**: кожна папка архіву стає локальною папкою під назвою архіву, наприклад
  `Archivo de datos de Outlook/Bandeja de entrada`. Ці папки є лише на цьому комп'ютері.
- **Календар**: кожен календар стає локальним календарем. Повторювані зустрічі приходять як серії
  разом зі скасованими й перенесеними випадками, нагадуваннями й часовим поясом.
- **Контакти**: кожна папка контактів стає адресною книгою (адреси, телефони, поштові адреси,
  компанія, посада, день народження, нотатки).
- Якщо імпортувати той самий файл удруге, дублікатів не буде.
- Непотрібну папку архіву можна видалити: правою кнопкою → **Delete folder** (Ctrl+Z повертає).

### 4. Чого поки не переноситься
- **Вкладення** листів з `.pst` і `.msg`: видно їхні назви й розміри, але самих файлів Correo
  поки не зберігає. Збережіть потрібні вкладення з Outlook або залиште собі файл `.pst`.
- Завдання, нотатки, групи контактів і список учасників зустрічей (у рядку стану видно, скільки
  їх пропущено).
- Підписи й правила Outlook: створіть їх заново в Correo (налаштування облікового запису та
  **Правила**).
- Листи, які писали саме в RTF, показуються простим текстом (слова всі, оформлення ні).

### 5. Перевірте
У рядку стану після імпорту видно, скільки листів, подій і контактів додано. Порівняйте з
кількістю в Outlook (у кожній папці Outlook показує число елементів унизу вікна). Не видаляйте
`.pst`, доки не переконаєтеся, що все на місці.

### 6. Як забрати все з Correo
Ваші дані не замкнені в програмі. Команда експорту записує все в нову папку
`Tinta Correo <дата>`, нічого не перезаписуючи:
- `Mail/<обліковий запис>/<папка>.mbox`: пошта кожної папки у форматі mbox, який імпортують
  Thunderbird (додаток ImportExportTools NG), Apple Mail та Evolution;
- `Calendars/<календар>.ics`: календарі для Outlook, Google Calendar, Apple Calendar;
- `Contacts/<адресна книга>.vcf`: контакти для Outlook, Google Contacts, Apple Contacts.

У вікні: кнопка **Експортувати все** внизу лівої смужки, над шестернею, → **Експортувати**. Папка
з'являється в «Документах»; **Відкрити папку експорту** відкриває її. Те саме з командного рядка:

```sh
ibercorreo-cli export --store "%APPDATA%\Iber Correo\store.json" "%USERPROFILE%\Documents"
```

Окрему папку можна забрати як `.mbox` командою `folder.exportMbox`, календар командою
`calendar.exportIcs`, контакти командою `contact.exportVcf`.

## En español

### 1. Qué hay que pasar
- **El correo que está en un servidor** (Gmail, Outlook.com, Microsoft 365, Exchange o cualquier
  correo IMAP) no hace falta pasarlo: añada la misma cuenta en Tinta Correo y los mensajes, las
  carpetas, el calendario y los contactos aparecen solos.
- **Un archivo `.pst`** hace falta para lo que solo está en su ordenador: archivos de Outlook,
  carpetas locales, cuentas POP3 antiguas, el calendario y los contactos locales.
- **Los mensajes `.msg`** sueltos se abren en Correo uno a uno.

### 2. Exportar desde Outlook a `.pst`
En Outlook clásico para Windows: **Archivo → Abrir y exportar → Importar o exportar → Exportar a un
archivo → Archivo de datos de Outlook (.pst)**. Elija la parte de arriba de la cuenta (su
dirección), marque **Incluir subcarpetas** y guarde el archivo, por ejemplo en el escritorio.
- El nuevo Outlook para Windows y Outlook para Mac no crean archivos `.pst`. Si su correo está en un
  servidor, añada la cuenta (paso 1); si no, exporte desde Outlook clásico.
- Correo no lee los archivos `.ost` (la caché de una cuenta de Exchange o Microsoft 365): conviértalos
  en `.pst` como se explica arriba.

### 3. Importar en Tinta Correo
Haga clic derecho en el `.pst` → **Abrir con** → **Tinta Correo**, o arrástrelo a la ventana. Un
archivo grande se lee en segundo plano, con el progreso en la barra de estado.
- **Correo**: cada carpeta pasa a una carpeta local con el nombre del archivo delante, por ejemplo
  `Archivo de datos de Outlook/Bandeja de entrada`. Esas carpetas están solo en este ordenador.
- **Calendario**: cada calendario pasa a un calendario local, con las series periódicas, sus
  excepciones, los avisos y la zona horaria.
- **Contactos**: cada carpeta de contactos pasa a una libreta de direcciones.
- Si importa el mismo archivo otra vez, no se duplica nada.
- Una carpeta que no quiera se borra con clic derecho → **Delete folder** (Ctrl+Z la recupera).

### 4. Lo que todavía no se pasa
- **Los adjuntos** de los mensajes de `.pst` y `.msg`: se ven su nombre y tamaño, pero Correo aún
  no guarda los archivos. Guarde los adjuntos que necesite desde Outlook o conserve el `.pst`.
- Tareas, notas, grupos de contactos y la lista de asistentes de las reuniones (la barra de estado
  dice cuántos elementos se han dejado fuera).
- Las firmas y las reglas de Outlook: créelas de nuevo en Correo.
- Los mensajes escritos en RTF se muestran como texto sin formato.

### 5. Compruebe
Tras importar, la barra de estado dice cuántos mensajes, eventos y contactos se han añadido.
Compárelo con Outlook y no borre el `.pst` hasta estar seguro de que está todo.

### 6. Cómo sacar todo de Correo
Sus datos no quedan encerrados. La exportación escribe todo en una carpeta nueva
`Tinta Correo <fecha>`, sin sobrescribir nada: `Mail/…/*.mbox` (Thunderbird con ImportExportTools
NG, Apple Mail, Evolution), `Calendars/*.ics` (Outlook, Google Calendar, Apple Calendar) y
`Contacts/*.vcf` (Outlook, Google Contacts, Apple Contacts). En la ventana: el botón **Exportar todo**
abajo en la barra izquierda, sobre el engranaje, → **Exportar**; la carpeta aparece en Documentos y
**Abrir la carpeta exportada** la abre. Lo mismo desde la línea de comandos:

```sh
ibercorreo-cli export --store "%APPDATA%\Iber Correo\store.json" "%USERPROFILE%\Documents"
```

## In English

### 1. What to move
- **Mail kept on a server** (Gmail, Outlook.com, Microsoft 365, Exchange, any IMAP mail) needs no
  moving: add the same account to Tinta Correo and the mail, folders, calendar and contacts arrive
  on their own.
- **An Outlook data file (`.pst`)** is for what lives only on your computer: archives, local
  folders, old POP3 accounts, a local calendar and contacts.
- **Single `.msg` messages** open in Correo one by one.

### 2. Export from Outlook to `.pst`
In classic Outlook for Windows: **File → Open & Export → Import/Export → Export to a file → Outlook
Data File (.pst)**. Pick the top of the account (your address), tick **Include subfolders** and
save the file, for example on the desktop.
- New Outlook for Windows and Outlook for Mac don't make `.pst` files. If your mail is on a server,
  add the account (step 1); otherwise export from classic Outlook.
- Correo doesn't read `.ost` files (an Exchange or Microsoft 365 account's cache): export a `.pst`
  as above.

### 3. Import into Tinta Correo
Right-click the `.pst` → **Open with** → **Tinta Correo**, or drag it onto the window. A large file
is read in the background, with progress in the status bar.
- **Mail**: each folder becomes a local folder named after the archive, such as
  `Outlook Data File/Inbox`. These folders exist only on this computer.
- **Calendar**: each calendar becomes a local calendar, with recurring series, their exceptions,
  reminders and time zone.
- **Contacts**: each contacts folder becomes an address book.
- Importing the same file again adds nothing twice.
- An unwanted folder goes with right-click → **Delete folder** (Ctrl+Z brings it back).

### 4. Not moved yet
- **Attachments** of `.pst` and `.msg` messages: their names and sizes show, but Correo doesn't keep
  the files yet. Save the attachments you need from Outlook, or keep the `.pst`.
- Tasks, notes, contact groups and meeting attendee lists (the status bar says how many items were
  left out).
- Outlook signatures and rules: set them up again in Correo.
- Mail written in RTF shows as plain text.

### 5. Check
After the import the status bar says how many messages, events and contacts were added. Compare
with Outlook, and keep the `.pst` until you are sure everything is there.

### 6. Taking everything out of Correo
Your data is not locked in. The export writes everything to a new folder `Tinta Correo <date>`
and never overwrites anything: `Mail/<account>/<folder>.mbox` (Thunderbird with ImportExportTools
NG, Apple Mail, Evolution), `Calendars/*.ics` (Outlook, Google Calendar, Apple Calendar) and
`Contacts/*.vcf` (Outlook, Google Contacts, Apple Contacts). In the window: the **Export everything**
button at the foot of the left rail, above the gear, → **Export**; the folder appears in Documents
and **Open the exported folder** opens it. The same from the command line:

```sh
ibercorreo-cli export --store "%APPDATA%\Iber Correo\store.json" "%USERPROFILE%\Documents"
```

On macOS the store is `~/Library/Application Support/Iber Correo/store.json`, on Linux
`~/.local/share/ibercorreo/store.json`. One folder exports as mbox text with the command
`folder.exportMbox`, a calendar with `calendar.exportIcs`, contacts with `contact.exportVcf`.

## Revision history
- 2026-11-12 — created (M5.3): moving from Outlook, and export, in Ukrainian, Spanish and English.
- 2026-11-13 — the Export everything button in the window.
