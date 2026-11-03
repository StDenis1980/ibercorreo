use super::*;
use crate::testing::pst::{Options, Writer};

/// MS-PST 3.5 "Sample Leaf BBT Page": the eight entries, then the page's last 32 bytes.
const SAMPLE_BBT: &str = "\
68 11 00 00 00 00 00 00 0B 02 00 00 00 00 00 00 00 28 7C 00 00 00 00 00 B0 11 00 00 00 00 00 00 \
0C 02 00 00 00 00 00 00 00 08 80 00 00 00 00 00 F8 11 00 00 00 00 00 00 0D 02 00 00 00 00 00 00 \
00 0A 80 00 00 00 00 00 40 12 00 00 00 00 00 00 0F 02 00 00 00 00 00 00 00 E6 83 00 00 00 00 00 \
88 12 00 00 00 00 00 00 10 02 00 00 00 00 00 00 00 C6 87 00 00 00 00 00 D0 12 00 00 00 00 00 00 \
11 02 00 00 00 00 00 00 00 C8 87 00 00 00 00 00 18 13 00 00 00 00 00 00 12 02 00 00 00 00 00 00 \
00 A6 8B 00 00 00 00 00 86 13 00 00 00 00 00 00 44 02 00 00 00 00 00 00 00 FC 8F 00 00 00 00 00";
const SAMPLE_BBT_END: &str = "\
00 00 00 00 00 00 00 00 08 14 18 00 00 00 00 00 80 80 D6 00 2F A0 F6 A1 46 02 00 00 00 00 00 00";

fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace().map(|h| u8::from_str_radix(h, 16).unwrap()).collect()
}

#[test]
fn the_specification_sample_page_reads() {
    let mut file = Writer::new(Options::default()).archive("x", &[]);
    file.resize(file.len().div_ceil(512) * 512, 0);
    let at = file.len() as u64;
    let mut page = hex(SAMPLE_BBT);
    page.resize(480, 0);
    page.extend(hex(SAMPLE_BBT_END));
    assert_eq!(page.len(), 512);
    file.extend(page);
    let ndb = Ndb::open(file).unwrap();
    let (level, entries) = ndb.page(at, PTYPE_BBT).unwrap();
    assert_eq!((level, entries.len()), (0, 8));
    let e = ndb.find(at, PTYPE_BBT, 0x11b0).unwrap().unwrap();
    assert_eq!((ndb.id(&e, 8), u16_at(&e, 16), u16_at(&e, 18)), (Some(0x20c), Some(0x0800), Some(0x80)), "ib, cb, cRef");
    // A block id's reserved low bit is not part of its key.
    assert_eq!(ndb.find(at, PTYPE_BBT, 0x1386).unwrap().and_then(|e| ndb.id(&e, 8)), Some(0x244));
    assert_eq!(ndb.find(at, PTYPE_BBT, 0x1388).unwrap(), None);
    assert!(ndb.page(at, PTYPE_NBT).is_err(), "a page of the other tree");
}

#[test]
fn the_encodings_follow_the_specification_tables() {
    // The third permutation undoes the first; the second undoes itself.
    for b in 0..=255u8 {
        assert_eq!(table(2, table(0, b)), b);
        assert_eq!(table(1, table(1, b)), b);
    }
    let plain: Vec<u8> = (0..=255u8).chain(0..=255).collect();
    let mut x: Vec<u8> = plain.iter().map(|&b| table(0, b)).collect();
    permute(&mut x);
    assert_eq!(x, plain);
    let mut c = plain.clone();
    cyclic(&mut c, 0x1234_5678);
    assert_ne!(c, plain);
    cyclic(&mut c, 0x1234_5678);
    assert_eq!(c, plain, "the cyclic encoding is its own inverse");
}

#[test]
fn files_shorter_than_their_structures_are_refused() {
    let good = Writer::new(Options::default()).archive("x", &[]);
    for cut in [0, 4, 100, 300, 600] {
        assert!(Ndb::open(good[..cut].to_vec()).and_then(|n| n.node(0x21)).is_err(), "{cut}");
    }
}
