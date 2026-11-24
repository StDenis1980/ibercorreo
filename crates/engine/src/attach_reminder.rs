//! The forgotten-attachment reminder (Tinta), as in Outlook: a draft whose own words say a file
//! comes with it ("attached", "adjunto", "у вкладенні"…) but which carries no file. The window
//! asks before sending it; `message.read` says so for a draft (`forgotAttachment`), so an
//! assistant can check too. Sending is never refused: the person decides.
//!
//! Only what the person wrote counts: the text above the quoted message of a reply or forward
//! (lines starting with `>` and the "… wrote:" line before them), and the subject unless it is a
//! reply's or forward's (`Re:`, `RV:`…), whose words were the original sender's.

use ibercorreo_core::Message;

/// Word beginnings that say a file is attached, lower case, in the interface's languages: English,
/// Ukrainian, Spanish, Catalan, Galician, Basque, Italian, French, German, Polish, Romanian, Czech
/// and Lithuanian. A word matches when it starts with one (`adjunt` finds `adjunto`, `adjuntamos`).
const STEMS: &[&str] = &[
    // English
    "attach",
    "enclosed",
    // Ukrainian
    "вкладен",
    "вкладаю",
    "прикріп",
    "у додатку",
    "в додатку",
    "додаю файл",
    // Spanish, Catalan, Galician
    "adjunt",
    "anexo",
    "anexa",
    "annex",
    "adxunt",
    // Basque
    "erantsi",
    "eransk",
    // Italian
    "allegat",
    "allego",
    // French
    "pièce jointe",
    "pièces jointes",
    "ci-joint",
    // German
    "anhang",
    "angehängt",
    "anbei",
    "beigefügt",
    // Polish
    "załącz",
    // Romanian (with comma and cedilla s)
    "atașa",
    "atașez",
    "ataşa",
    "ataşez",
    "anexez",
    "anexă",
    // Czech
    "přílo",
    "přiklád",
    // Lithuanian
    "priseg",
    "priede",
    "pridedu",
];

/// Lines that start the quoted message in replies from Correo and other programs.
const QUOTE_HEADS: &[&str] = &[
    "-----original message-----",
    "-----mensaje original-----",
    "-----missatge original-----",
    "-----вихідне повідомлення-----",
    "---------- forwarded message",
    "---------- mensaje reenviado",
];
/// Ends of the "On …, … wrote:" line before a quote.
const WROTE: &[&str] =
    &["wrote:", "escribió:", "написав:", "написала:", "написав(ла):", "a écrit :", "schrieb:", "ha scritto:", "va escriure:", "escribiu:"];

/// Most bytes of the text looked at: a reminder is about the words at the top.
const MAX_BYTES: usize = 20_000;

/// The text the person wrote: `body` down to the quoted message.
fn own_text(body: &str) -> String {
    let mut out = String::new();
    for line in body.lines() {
        let l = line.trim().to_lowercase();
        if l.starts_with('>') || QUOTE_HEADS.iter().any(|h| l.starts_with(h)) || WROTE.iter().any(|w| l.ends_with(w)) {
            break;
        }
        out.push_str(&l);
        out.push('\n');
        if out.len() > MAX_BYTES {
            break;
        }
    }
    out
}

/// `text` (lower case) names an attachment: one of [`STEMS`] starts a word in it.
fn names_attachment(text: &str) -> bool {
    STEMS.iter().any(|stem| {
        text.match_indices(stem).any(|(at, _)| text.get(..at).and_then(|before| before.chars().next_back()).is_none_or(|c| !c.is_alphanumeric()))
    })
}

/// `subject` and `body` say a file is attached (see the module's notes on what counts).
pub fn mentions_attachment(subject: &str, body: &str) -> bool {
    let subject = subject.trim().to_lowercase();
    let collapsed = subject.split_whitespace().collect::<Vec<_>>().join(" ");
    let reply = ibercorreo_core::mail::conversation_topic(&subject).is_some_and(|topic| topic != collapsed);
    (!reply && names_attachment(&collapsed)) || names_attachment(&own_text(body))
}

/// `m`, a draft whose text is `body`, says it has a file but carries none.
pub fn forgot_attachment(m: &Message, body: &str) -> bool {
    m.attachments.is_empty() && mentions_attachment(&m.subject, body)
}

#[cfg(test)]
mod tests {
    use super::mentions_attachment as says;

    #[test]
    fn words_that_promise_a_file_in_each_language() {
        for body in [
            "Hi Ana,\nplease find the invoice attached.",
            "I attach the report.",
            "Добрий день!\nУ вкладенні рахунок за жовтень.",
            "Прикріплюю договір.",
            "Hola, te adjunto la factura.",
            "Us envio adjunta la proposta.",
            "Envíolle o informe adxunto.",
            "Faktura erantsita doa.",
            "In allegato trovi il preventivo.",
            "Vous trouverez ci-joint le devis.",
            "Veuillez trouver la pièce jointe.",
            "Die Rechnung ist im Anhang.",
            "Anbei die Unterlagen.",
            "W załączniku przesyłam fakturę.",
            "Vă trimit atașat oferta.",
            "Posílám v příloze fakturu.",
            "Prisegu sąskaitą.",
        ] {
            assert!(says("", body), "{body}");
        }
    }

    #[test]
    fn other_words_and_quoted_text_do_not() {
        assert!(!says("Lunch on Friday?", "See you at two."));
        assert!(!says("", "The team is very attentive."), "attentive is not attach");
        assert!(!says("", "Unattached and reattached."), "a word that only contains it");
        let reply = "Thanks, got it.\n\nOn 2026-11-20, Ana <ana@example.es> wrote:\n> Te adjunto la factura.\n";
        assert!(!says("Re: Factura adjunta", reply), "the original's words, in the quote and the Re: subject");
        assert!(!says("", "Gracias.\n\n-----Mensaje original-----\nTe adjunto el contrato."));
        assert!(says("", "Te adjunto la mía.\n\nOn 2026-11-20, Ana wrote:\n> Hola"), "above the quote counts");
    }

    #[test]
    fn the_subject_counts_unless_it_is_a_reply() {
        assert!(says("Factura adjunta", ""));
        assert!(says("Attached: the October invoice", ""));
        assert!(!says("RV: Factura adjunta", ""));
        assert!(!says("Fwd: attached files", ""));
    }

    #[test]
    fn a_huge_text_is_looked_at_only_at_the_top() {
        let body = format!("{}\nattached", "word ".repeat(10_000));
        assert!(!says("", &body));
        assert!(!says(&"Re: ".repeat(1000), &"> ".repeat(100_000)));
    }
}
