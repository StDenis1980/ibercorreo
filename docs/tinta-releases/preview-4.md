# Tinta Correo — Preview 4

> **Status:** 2026-11-24 · draft for the Preview 4 release (10 December 2026), version 0.3.0 ·
> **Revision history:** 2026-11-16 created (changes from Preview 3, v0.2.0 of 12 November, to #86); 2026-11-17 added the suite-wide Tinta Office look (pdfcraft#60, wordcraft#75, gridcraft#55, deckcraft#38); 2026-11-18 added #90 (attachments in drafts, search index on disk, faster Send, Outlook.com Sent copy, layout fixes); 2026-11-24 added #92 (a new account's Inbox first) #93 (asking before a deceptive link; merged cells and cell backgrounds in tables) #94 (All Inboxes) #95 (attachment files from `.pst` and `.msg`, and in the mbox export) #96 (the forgotten-attachment reminder) #97 (date words and folders in search) #98 (dragging appointments in the calendar) and #99 (work hours in the calendar).
> Updated until the freeze on 7 December.

Release notes for the GitHub release and the site, in Spanish, Ukrainian and English. Changes since
Preview 3 (v0.2.0, 12 November 2026), version 0.3.0.

## Español

**Tinta Correo — vista previa 4.** Versión de prueba: conserve también su programa de correo
actual.

Novedades:

- **Nuevo aspecto:** temas claro y oscuro, tamaño del texto, árbol de carpetas, diálogos y selección
  renovados.
- **Todas las bandejas de entrada:** con varias cuentas, una carpeta en Favoritos reúne la Bandeja
  de entrada de todas; al responder se usa la cuenta que recibió el mensaje.
- **Conversaciones** agrupadas en la lista de mensajes, y **categorías de colores** como en Outlook.
- **Correo HTML más fiel:** tablas (también con celdas combinadas y su color de fondo), colores y
  tamaños de letra; las imágenes incluidas se ven, y las de internet solo cuando usted lo pide.
- **Enlaces engañosos:** si el texto de un enlace nombra un sitio pero lleva a otro, Tinta Correo
  muestra la dirección real y pregunta antes de abrirlo.
- **Adjuntos:** abrir y guardar desde el panel de lectura, y adjuntar archivos al escribir (botón
  Adjuntar archivo o arrastrándolos a la ventana); al reenviar se conservan los adjuntos.
- **Adjuntos de Outlook:** al importar un `.pst` o un `.msg` se conservan también los archivos
  adjuntos, que se abren y guardan como los del correo nuevo; la exportación a mbox los incluye.
- **Adjunto olvidado:** si el mensaje dice que lleva un archivo («te adjunto», «en el anexo»…) pero
  no tiene ninguno, Tinta Correo pregunta antes de enviarlo.
- **Buscar por fecha y carpeta:** `recibido:ayer`, `recibido:"semana pasada"`, `"este mes"`…, y
  `carpeta:enviados` o `in:inbox` para buscar solo en una carpeta.
- **Arrastrar citas en el calendario:** en las vistas de día y semana, arrastre una cita a otra hora
  u otro día, o su borde inferior para que termine antes o después (de 15 en 15 minutos; Ctrl+Z lo
  deshace). Las citas periódicas y de todo el día aún no se arrastran.
- **Horario laboral en el calendario:** elija con 🕘 cuándo empieza y termina su jornada (por
  defecto 08:00–17:00); las vistas de día y semana se abren una hora antes y sombrean el resto.
- **Más rápido con buzones grandes:** el correo antiguo se descarga primero con su encabezado, y el
  texto al abrirlo (también con Exchange y Microsoft 365). El índice de búsqueda se guarda en disco:
  usa unas 10 veces menos memoria y la búsqueda está lista nada más abrir el programa.
- **Enviar** empieza a enviar al instante, y con Outlook.com ya no aparece una segunda copia en
  Enviados.
- **Al añadir una cuenta grande**, la Bandeja de entrada llega primero y se ve en minutos; las
  demás carpetas siguen justo después.
- **Exportar todo** a mbox, iCalendar y vCard con un botón, y una guía para pasar de Outlook.
- **Borrar sin sustos:** al borrar desde la papelera se pide confirmación antes de eliminarlo para siempre.
- Corregidos solapamientos en la lista de mensajes y en la barra de estado.
- Ayuda cuando su organización tiene que aprobar la aplicación en Microsoft 365.
- **El mismo aspecto en toda la suite:** Tinta PDF, Texto, Calculo y Presenta adoptan ahora el
  aspecto de Tinta Correo.

Limitaciones conocidas: Google aún no ha verificado la aplicación, así que al iniciar sesión con Google
verá un aviso (o use una contraseña de aplicación); aún no se puede exportar a .pst; Exchange propio
(EWS) aún no se ha probado con un servidor real.

## Українська

**Tinta Correo — попередня версія 4.** Тестова версія: тримайте також свою поточну поштову програму.

Що нового:

- **Новий вигляд:** світла й темна теми, розмір тексту, оновлені дерево папок, діалоги й виділення.
- **Усі вхідні:** якщо акаунтів кілька, папка у «Вибраному» збирає «Вхідні» з усіх; відповідь
  іде з того акаунта, на який прийшов лист.
- **Розмови** згруповані в списку листів, і **кольорові категорії**, як в Outlook.
- **HTML-листи ближче до оригіналу:** таблиці (також з об'єднаними клітинками та їхнім фоном),
  кольори й розміри шрифту; вбудовані зображення видно, а зображення з інтернету — лише коли ви
  попросите.
- **Оманливі посилання:** якщо текст посилання називає один сайт, а веде воно на інший, Tinta Correo
  показує справжню адресу й питає, перш ніж відкрити.
- **Вкладення:** відкрити й зберегти прямо з панелі читання, а також додати файли до листа (кнопка
  «Вкласти файл» або перетягнути їх у вікно); під час пересилання вкладення зберігаються.
- **Вкладення з Outlook:** під час імпорту `.pst` або `.msg` зберігаються й самі файли вкладень,
  їх можна відкрити й зберегти, як у новій пошті; експорт у mbox теж їх містить.
- **Забуте вкладення:** якщо в листі сказано, що до нього додано файл («у вкладенні», «додаю
  файл»…), а вкладення немає, Tinta Correo запитає перед надсиланням.
- **Пошук за датою й папкою:** `отримано:вчора`, `отримано:"минулого тижня"`, `"цього місяця"`…,
  і `папка:надіслані` чи `in:inbox`, щоб шукати лише в одній папці.
- **Перетягування зустрічей у календарі:** у вигляді дня й тижня зустріч можна перетягнути на
  інший час чи день, а її нижній край — щоб вона закінчувалася раніше чи пізніше (крок 15 хвилин;
  Ctrl+Z повертає). Повторювані зустрічі й події на весь день поки не перетягуються.
- **Робочі години в календарі:** кнопкою 🕘 виберіть, коли починається й закінчується робочий
  день (типово 08:00–17:00); вигляд дня й тижня відкривається за годину до початку, решта затінена.
- **Швидше з великими скриньками:** старі листи спершу завантажуються заголовками, а текст — коли
  ви їх відкриваєте (також для Exchange і Microsoft 365). Пошуковий індекс зберігається на диску:
  пам'яті потрібно приблизно вдесятеро менше, а пошук готовий одразу після запуску.
- **«Надіслати»** починає надсилання одразу, а з Outlook.com у «Надісланих» більше немає другої
  копії листа.
- **Після додавання великої скриньки** спершу приходять «Вхідні», і їх видно за кілька хвилин;
  інші папки підтягуються одразу слідом.
- **Експорт усього** в mbox, iCalendar і vCard однією кнопкою та посібник із переходу з Outlook.
- **Видалення без несподіванок:** видалення з кошика спершу питає, чи стерти лист назавжди.
- Виправлено накладання в списку листів і в рядку стану.
- Підказка, коли вашій організації треба схвалити програму в Microsoft 365.
- **Однаковий вигляд у всьому пакеті:** Tinta PDF, Texto, Calculo й Presenta тепер мають вигляд
  Tinta Correo.

Відомі обмеження: Google ще не перевірив застосунок, тому під час входу через Google буде
попередження (або скористайтеся паролем застосунку); експорту в .pst ще немає; власний Exchange
(EWS) ще не перевірено на справжньому сервері.

## English

**Tinta Correo — Preview 4.** A test version: keep your current mail app as well.

What's new:

- **A new look:** light and dark themes, text size, and a refreshed folder tree, dialogs and selection.
- **All Inboxes:** with several accounts, one folder in Favorites gathers every account's Inbox;
  a reply goes from the account the message came to.
- **Conversations** grouped in the message list, and **colour categories** as in Outlook.
- **HTML mail closer to the original:** tables (merged cells and cell backgrounds too), colours and
  text sizes; pictures inside the message show, and pictures from the web only when you ask.
- **Deceptive links:** when a link's text names one site but it goes to another, Tinta Correo shows
  the real address and asks before opening it.
- **Attachments:** open and save them from the reading pane, and attach files while writing (the
  Attach file button, or drop files on the window); forwarding keeps the attachments.
- **Outlook attachments:** importing a `.pst` or `.msg` keeps the attached files too, so they open
  and save like those of new mail; the mbox export includes them.
- **Forgotten attachment:** when a message says a file is attached ("attached", "enclosed"…) but
  has none, Tinta Correo asks before sending it.
- **Search by date and folder:** `received:yesterday`, `received:"last week"`, `"this month"`…, and
  `in:sent` or `folder:Projects` to search one folder only.
- **Drag appointments in the calendar:** in the day and week views, drag an appointment to another
  time or day, or its bottom edge to end it earlier or later (15-minute steps; Ctrl+Z undoes it).
  Repeating and all-day appointments don't drag yet.
- **Work hours in the calendar:** choose with 🕘 when your working day starts and ends (08:00–17:00
  by default); the day and week views open an hour before and shade the rest.
- **Faster with large mailboxes:** older mail is downloaded header first, and its text when you
  open it (with Exchange and Microsoft 365 too). The search index is kept on disk, so it uses about
  ten times less memory and search is ready as soon as the app starts.
- **Send** starts sending at once, and Outlook.com no longer gets a second copy in Sent.
- **Adding a big account:** its Inbox comes first and shows within minutes; the other folders follow
  right after.
- **Export everything** to mbox, iCalendar and vCard with one button, and a guide to move from Outlook.
- **Safer delete:** deleting from Deleted Items asks before it deletes for good.
- Fixed overlaps in the message list and the status bar.
- Help when your organisation has to approve the app in Microsoft 365.
- **One look across the suite:** Tinta PDF, Texto, Calculo and Presenta now take Tinta Correo's
  look.

Known limits: Google has not verified the app yet, so Google sign-in shows a warning (or use an app
password); no export to .pst yet; an organisation's own Exchange server (EWS) has not been tested
against a real server yet.
