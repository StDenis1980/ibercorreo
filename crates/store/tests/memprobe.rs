//! How much memory a big mailbox takes, and how long it takes to save, open, index and read:
//! a synthetic mailbox the size of a real one (67,000 messages, about 640 MB as JSON). Not run by
//! default (it takes a minute and writes to the temp folder); on Linux:
//!
//! ```text
//! cargo test --release -p ibercorreo-store --test memprobe -- --ignored --nocapture
//! LOAD_ONLY=1 cargo test --release -p ibercorreo-store --test memprobe -- --ignored --nocapture
//! ```
//!
//! The second run opens what the first saved in a fresh process, which is what the app does.

#![allow(clippy::unwrap_used)]
use ibercorreo_core::rich::{Block, BlockKind, Rich, Span};
use ibercorreo_core::*;
use ibercorreo_store::Store;

/// Resident memory in MiB (0 where `/proc` is not there).
fn rss() -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines().find(|l| l.starts_with("VmRSS")).and_then(|l| l.split_whitespace().nth(1)).and_then(|kb| kb.parse::<u64>().ok()).unwrap_or(0) / 1024
}

#[test]
#[ignore]
fn memprobe() {
    if std::env::var("LOAD_ONLY").is_ok() {
        let path = std::env::temp_dir().join("memprobe").join("store.json");
        let base = rss();
        let s = Store::load(&path).unwrap();
        let loaded = rss();
        eprintln!("fresh process: loaded store {} MB", loaded.saturating_sub(base));
        (s.search_warmup())();
        eprintln!("fresh process: index {} MB, total rss {} MB", rss().saturating_sub(loaded), rss());
        return;
    }
    let n: u64 = std::env::var("N").ok().and_then(|v| v.parse().ok()).unwrap_or(67000);
    let mut s = Store::new();
    let acc = s.first_account();
    let folder = s.folder_by_role(acc, FolderRole::Inbox).unwrap();
    let words = ["presupuesto", "reunión", "lunes", "factura", "cliente", "proyecto", "entrega", "informe", "valencia", "oferta"];
    let base = rss();
    let mut json_bytes = 0usize;
    for i in 0..n {
        let id = s.next_message_id();
        let mut m = Message::new(id, folder);
        m.subject = format!("Asunto {i} sobre {}", words[(i % 10) as usize]);
        m.from = Some(Address { name: Some(format!("Persona {}", i % 500)), email: format!("p{}@ex{}.com", i % 500, i % 37) });
        let mut blocks = Vec::new();
        let mut text = String::new();
        for b in 0..40u64 {
            let mut spans = Vec::new();
            for k in 0..3u64 {
                let t = format!(
                    "{} {} {} w{}x{} ",
                    words[((i + b + k) % 10) as usize],
                    words[((i * 7 + b) % 10) as usize],
                    i * 13 + b * 3 + k,
                    i % 9973,
                    b
                );
                text.push_str(&t);
                spans.push(Span {
                    text: t,
                    bold: k == 1,
                    link: if k == 2 { Some(format!("https://ex{}.com/track/{i}/{b}", i % 37)) } else { None },
                    ..Default::default()
                });
            }
            text.push('\n');
            blocks.push(Block { kind: BlockKind::Paragraph, quote: 0, spans });
        }
        let c = m.content_mut().unwrap();
        c.body = text;
        c.rich = if i % 3 != 0 { Some(Rich { blocks, remote_images: 2 }) } else { None };
        json_bytes += serde_json::to_vec(&m).unwrap().len();
        s.add_message(m).unwrap();
    }
    let loaded = rss();
    eprintln!("json {} MB; messages {} MB", json_bytes >> 20, loaded.saturating_sub(base));
    let dir = std::env::temp_dir().join("memprobe");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("store.json");
    let t = std::time::Instant::now();
    s.save(&path).unwrap();
    eprintln!("save {:?}", t.elapsed());
    drop(s);
    let before = rss();
    let t = std::time::Instant::now();
    let s = Store::load(&path).unwrap();
    eprintln!("load {:?}; loaded store {} MB (rss {} MB)", t.elapsed(), rss().saturating_sub(before), rss());
    let loaded = rss();
    let t = std::time::Instant::now();
    (s.search_warmup())();
    eprintln!("index {} MB in {:?}", rss().saturating_sub(loaded), t.elapsed());
    let t = std::time::Instant::now();
    let hits = s.search("valencia lunes").len();
    eprintln!("search {hits} in {:?}", t.elapsed());
    let id = s.all_messages().nth(40000).unwrap().id;
    let t = std::time::Instant::now();
    let c = s.content(id).unwrap();
    eprintln!("open one {:?} ({} bytes)", t.elapsed(), c.body.len());
}
