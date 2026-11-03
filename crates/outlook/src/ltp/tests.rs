use super::*;
use crate::ndb::Ndb;
use crate::props::{PT_LONG, PT_UNICODE, utf16};
use crate::testing::pst::{Options, Writer};

/// MS-PST 3.11 "Sample TC": a folder's hierarchy table from a real file, one heap block.
const SAMPLE_TC: &[&str] = &[
    "BC 01 EC 7C 40 00 00 00 00 00 00 00 B5 04 04 00",
    "60 00 00 00 7C 0D 34 00 34 00 35 00 37 00 20 00",
    "00 00 80 00 00 00 00 00 00 00 02 01 30 0E 14 00",
    "04 06 14 00 33 0E 18 00 08 07 02 01 34 0E 20 00",
    "04 08 03 00 38 0E 24 00 04 09 1F 00 01 30 08 00",
    "04 02 03 00 02 36 0C 00 04 03 03 00 03 36 10 00",
    "04 04 0B 00 0A 36 34 00 01 05 1F 00 13 36 28 00",
    "04 0A 03 00 35 66 2C 00 04 0B 03 00 36 66 30 00",
    "04 0C 03 00 F2 67 00 00 04 00 03 00 F3 67 04 00",
    "04 01 23 22 00 00 02 00 00 00 22 80 00 00 00 00",
    "00 00 42 80 00 00 01 00 00 00 22 80 00 00 0E 00",
    "00 00 A0 00 00 00 00 00 00 00 00 00 00 00 00 00",
    "00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00",
    "00 00 00 00 00 00 00 00 00 00 00 00 00 00 01 FC",
    "00 42 80 00 00 06 00 00 00 C0 00 00 00 00 00 00",
    "00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00",
    "00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00",
    "00 00 00 00 00 00 FC 00 23 22 00 00 0B 00 00 00",
    "E0 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00",
    "00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00",
    "00 00 00 00 00 00 00 00 00 00 00 00 00 FC 00 54",
    "00 6F 00 70 00 20 00 6F 00 66 00 20 00 50 00 65",
    "00 72 00 73 00 6F 00 6E 00 61 00 6C 00 20 00 46",
    "00 6F 00 6C 00 64 00 65 00 72 00 73 00 53 00 65",
    "00 61 00 72 00 63 00 68 00 20 00 52 00 6F 00 6F",
    "00 74 00 53 00 50 00 41 00 4D 00 20 00 53 00 65",
    "00 61 00 72 00 63 00 68 00 20 00 46 00 6F 00 6C",
    "00 64 00 65 00 72 00 20 00 32 00 00 07 00 00 00",
    "0C 00 14 00 92 00 AA 00 4F 01 7D 01 93 01 BB 01",
];

fn sample() -> Vec<u8> {
    SAMPLE_TC.iter().flat_map(|l| l.split(' ')).map(|h| u8::from_str_radix(h, 16).unwrap()).collect()
}

#[test]
fn the_specification_sample_table_reads() {
    let ndb = Ndb::open(Writer::new(Options::default()).archive("x", &[])).unwrap();
    let heap = Heap::new(vec![sample()]).unwrap();
    assert_eq!((heap.client, heap.root), (TC_SIG, 0x40));
    // The row index: row id -> row number.
    let index: Vec<(u32, u32)> = bth(&heap, 0x20)
        .unwrap()
        .iter()
        .map(|(k, d)| (u32::from_le_bytes(k[..4].try_into().unwrap()), u32::from_le_bytes(d[..4].try_into().unwrap())))
        .collect();
    assert_eq!(index, [(0x2223, 2), (0x8022, 0), (0x8042, 1)]);
    let ctx = Context { ndb: &ndb, heap, subnodes: Vec::new() };
    let rows = ctx.tc(100).unwrap();
    let ids: Vec<u32> = rows.iter().map(|r| r.int(ROW_ID).unwrap()).collect();
    assert_eq!(ids, [0x8022, 0x8042, 0x2223]);
    let names: Vec<String> = rows.iter().map(|r| utf16(r.bytes(0x3001, PT_UNICODE).unwrap())).collect();
    assert_eq!(names, ["Top of Personal Folders", "Search Root", "SPAM Search Folder 2"]);
    assert_eq!(rows[0].fixed(0x3602, &[PT_LONG]), Some(0), "content count");
    assert_eq!(ctx.tc(2).unwrap().len(), 2, "the row limit");
}

#[test]
fn allocations_outside_the_heap_are_none() {
    let heap = Heap::new(vec![sample()]).unwrap();
    assert!(heap.get(0x20).is_some());
    for hid in [0, 0x1f, 0x21, 0x100, 0x1_0020, 0xffff_ffe0] {
        assert!(heap.get(hid).is_none(), "{hid:#x}");
    }
    assert!(Heap::new(Vec::new()).is_err());
    assert!(Heap::new(vec![vec![0; 16]]).is_err(), "no heap signature");
    // Every byte of the block changed, one at a time: nothing panics.
    let ndb = Ndb::open(Writer::new(Options::default()).archive("x", &[])).unwrap();
    let good = sample();
    for at in 0..good.len() {
        for v in [0u8, 0xff, 0x7f] {
            let mut b = good.clone();
            b[at] = v;
            if let Ok(heap) = Heap::new(vec![b]) {
                let _ = bth(&heap, 0x20);
                let ctx = Context { ndb: &ndb, heap, subnodes: Vec::new() };
                let _ = ctx.tc(1000);
                let _ = ctx.pc(&[]);
            }
        }
    }
}
