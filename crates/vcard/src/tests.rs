use super::*;

/// vCard 3.0 as Apple and Google export it: groups, several addresses, a photo, a postal address.
const V3: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nPRODID:-//Apple Inc.//iPhone OS 18.0//EN\r\n\
N:Souza;Ana;Maria;Dra.;\r\nFN:Ana Souza\r\nORG:Lusodata;Vendas\r\nTITLE:Diretora comercial\r\n\
item1.EMAIL;type=INTERNET;type=pref:ana@example.org\r\nEMAIL;TYPE=INTERNET,HOME:ana.souza@example.net\r\n\
EMAIL:not an address\r\nTEL;TYPE=CELL:+351 912 345 678\r\nTEL;type=WORK;type=VOICE:+351 210 000 000\r\n\
item2.ADR;type=WORK:;;Rua das Flores 12;Lisboa;;1200-195;Portugal\r\nitem2.X-ABLabel:_$!<Work>!$_\r\n\
NOTE:Prefere chamadas\\, não mensagens.\\nDesde 2019.\r\nBDAY:1985-04-23\r\nUID:ana-1@example.org\r\n\
PHOTO;ENCODING=b;TYPE=JPEG:/9j/4AAQSkZJRgABAQ\r\n AAAQABAAD\r\nEND:VCARD\r\n";

/// vCard 2.1 as Outlook exports it: bare types, quoted-printable in Windows-1252 with soft breaks.
const V21: &str = "BEGIN:VCARD\r\nVERSION:2.1\r\nN;CHARSET=Windows-1252;ENCODING=QUOTED-PRINTABLE:N=FA=F1ez;Jos=E9\r\n\
FN;CHARSET=Windows-1252;ENCODING=QUOTED-PRINTABLE:Jos=E9 N=FA=F1ez\r\n\
NOTE;ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8:Linha um=0D=0ALinha =\r\ndois\r\n\
TEL;WORK;VOICE:+34 910 000 000\r\nTEL;CELL:+34 600 000 000\r\nEMAIL;PREF;INTERNET:jose@example.es\r\n\
X-MS-OL-DEFAULT-POSTAL-ADDRESS:0\r\nEND:VCARD\r\n";

#[test]
fn reads_vcard_3_and_keeps_what_it_does_not_read() {
    let p = parse(V3).unwrap();
    assert_eq!(p.contacts.len(), 1);
    assert_eq!(p.skipped, 1, "the line that is not an address");
    let c = &p.contacts[0];
    assert_eq!(c.name, "Ana Souza");
    assert_eq!(c.uid, "ana-1@example.org");
    let emails: Vec<&str> = c.emails.iter().map(|a| a.email.as_str()).collect();
    assert_eq!(emails, ["ana@example.org", "ana.souza@example.net"]);
    assert_eq!(
        c.phones,
        [Phone { kind: "mobile".into(), number: "+351 912 345 678".into() }, Phone { kind: "work".into(), number: "+351 210 000 000".into() }]
    );
    assert_eq!(c.organization.as_deref(), Some("Lusodata, Vendas"));
    assert_eq!(c.title.as_deref(), Some("Diretora comercial"));
    assert_eq!(c.note, "Prefere chamadas, não mensagens.\nDesde 2019.");
    assert_eq!(c.birthday, Some(Date::new(1985, 4, 23).unwrap()));
    let kept: Vec<String> = c.kept.iter().filter_map(|l| property_name(l)).collect();
    assert_eq!(kept, ["N", "ADR", "X-ABLABEL", "PHOTO"]);
    assert!(c.kept.iter().any(|l| l == "PHOTO;ENCODING=b;TYPE=JPEG:/9j/4AAQSkZJRgABAQAAAQABAAD"), "unfolded: {:?}", c.kept);
}

#[test]
fn reads_outlook_vcard_2_1() {
    let p = parse(V21).unwrap();
    let c = &p.contacts[0];
    assert_eq!(c.name, "José Núñez", "Windows-1252 quoted-printable");
    assert_eq!(c.note, "Linha um\nLinha dois", "a soft line break joins the value");
    assert_eq!(c.phones.iter().map(|p| p.kind.as_str()).collect::<Vec<_>>(), ["work", "mobile"]);
    assert_eq!(c.emails[0].email, "jose@example.es");
    // Only N is spelled out in parts: a card with no FN takes its name from N.
    let only_n = parse("BEGIN:VCARD\nVERSION:4.0\nN:Koval;Olena;;;\nEND:VCARD\n").unwrap();
    assert_eq!(only_n.contacts[0].name, "Olena Koval");
    let only_email = parse("BEGIN:VCARD\nEMAIL:x@example.org\nEND:VCARD\nBEGIN:VCARD\nNOTE:nothing\nEND:VCARD\n").unwrap();
    assert_eq!(only_email.contacts[0].name, "x@example.org");
    assert_eq!(only_email.rejected.len(), 1);
}

#[test]
fn writes_vcard_3_that_reads_back_the_same() {
    let mut contacts = parse(V3).unwrap().contacts;
    contacts.extend(parse(V21).unwrap().contacts);
    let mut fresh = Contact::new("Олена Коваль; \"ТОВ\"");
    fresh.emails.push(Address::new(Some("Олена"), "olena@example.com.ua").unwrap());
    fresh.note = "Дуже довга нотатка ".repeat(10).trim_end().to_string();
    contacts.push(fresh);
    let text = write(&contacts);
    assert!(text.lines().all(|l| l.len() <= 76), "{text}");
    assert_eq!(text.matches("BEGIN:VCARD").count(), 3);
    assert!(text.contains("N:Souza;Ana;Maria;Dra.;"), "the parts of a name come back as they were");
    assert!(text.contains("N:\\\"ТОВ\\\";Олена Коваль\\;;;;") || text.contains("N:\"ТОВ\";Олена Коваль\\;;;;"), "{text}");
    let back = parse(&text).unwrap();
    assert_eq!(back.skipped, 0);
    for (a, b) in contacts.iter().zip(&back.contacts) {
        assert_eq!(
            (&a.name, &a.uid, &a.phones, &a.organization, &a.title, &a.note, a.birthday),
            (&b.name, &b.uid, &b.phones, &b.organization, &b.title, &b.note, b.birthday)
        );
        assert_eq!(a.emails.iter().map(|e| &e.email).collect::<Vec<_>>(), b.emails.iter().map(|e| &e.email).collect::<Vec<_>>());
        // A written `N` reads back as kept, so it stays the same next time too.
        let not_n = |k: &[String]| k.iter().filter(|l| property_name(l).as_deref() != Some("N")).cloned().collect::<Vec<_>>();
        assert_eq!(not_n(&a.kept), not_n(&b.kept), "kept lines go back unchanged");
        assert_eq!(b.kept.iter().filter(|l| property_name(l).as_deref() == Some("N")).count(), 1);
    }
}

#[test]
fn hostile_files_never_panic_and_stay_bounded() {
    let deep = "BEGIN:VCARD\n".repeat(100);
    let photo = format!("BEGIN:VCARD\nFN:Big\nPHOTO;ENCODING=b:{}\nX-A:1\nEND:VCARD\n", "A".repeat(MAX_KEPT_BYTES + 1));
    let many = format!("BEGIN:VCARD\nFN:Many\n{}END:VCARD\n", "TEL:1\nX-B:2\n".repeat(1000));
    for t in [
        "",
        "garbage",
        "END:VCARD",
        "BEGIN:VCARD",
        "BEGIN:VCARD\n:::\n;;;\nFN;X=\"a:b\":c\\\nN:;;;;;;;;;;;;;;;;;;;;;;;;\nEND:VCARD",
        "BEGIN:VCARD\nFN;ENCODING=QUOTED-PRINTABLE:=\n=Z=\nEND:VCARD",
        deep.as_str(),
    ] {
        let _ = parse(t);
    }
    let big = parse(&photo).unwrap();
    assert_eq!((big.contacts[0].kept.len(), big.skipped), (1, 1), "the photo over the cap is counted, the rest kept");
    let p = parse(&many).unwrap();
    assert_eq!(p.contacts[0].phones.len(), MAX_VALUES);
    assert_eq!(p.contacts[0].kept.len(), MAX_KEPT_LINES);
    assert!(p.skipped > 0);
}
