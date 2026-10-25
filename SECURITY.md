# Security policy

Tinta Correo reads mail from anyone and keeps passwords and sign-in tokens in the system's
credential store, so a message that crashes it, runs code, loads remote content without being
asked, leaks a credential or reaches a server the user did not set up is a security problem.

## Reporting a vulnerability

Write to **hola@tintaoffice.com** with the version (`ibercorreo-cli --version`), the operating
system and, if you can, a small `.eml`, `.ics` or `.vcf` file that shows the problem. Please do
not open a public issue first.

You will get an answer within 7 days. When the problem is confirmed, the fix ships in the next
release and the release notes credit you unless you ask otherwise.

## Supported versions

Only the latest release published on https://tintaoffice.com gets security fixes.
