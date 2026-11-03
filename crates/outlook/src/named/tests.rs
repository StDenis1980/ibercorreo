use super::*;
use crate::props::Val;

fn entry(id: u32, kind: u16, index: u16) -> Vec<u8> {
    [id.to_le_bytes().as_slice(), &kind.to_le_bytes(), &index.to_le_bytes()].concat()
}

#[test]
fn numbered_names_map_to_tags() {
    let guids = [PSETID_APPOINTMENT, PSETID_ADDRESS].concat();
    let entries = [
        entry(0x8208, 3 << 1, 0),       // the first GUID in the stream
        entry(0x8083, 4 << 1, 1),       // the second
        entry(0x1234, 1 << 1, 2),       // PS_MAPI
        entry(0x0001, 2 << 1, 3),       // PS_PUBLIC_STRINGS
        entry(0x0010, (3 << 1) | 1, 4), // a string name: left out
        entry(0x9999, 9 << 1, 5),       // a GUID the stream lacks: left out
        entry(0x0001, 0, 6),            // no GUID: left out
        vec![1, 2, 3],                  // a short tail: ignored
    ]
    .concat();
    let bag = Bag(vec![(GUID_STREAM, PT_BINARY, Val::Bytes(guids)), (ENTRY_STREAM, PT_BINARY, Val::Bytes(entries))]);
    let map = NameMap::from_bag(&bag);
    assert_eq!(map.tag(&PSETID_APPOINTMENT, 0x8208), Some(0x8000));
    assert_eq!(map.tag(&PSETID_ADDRESS, 0x8083), Some(0x8001));
    assert_eq!(map.tag(&PS_MAPI, 0x1234), Some(0x8002));
    assert_eq!(map.tag(&PS_PUBLIC_STRINGS, 1), Some(0x8003));
    assert_eq!(map.tag(&PSETID_APPOINTMENT, 0x0010), None);
    assert_eq!(map.tag(&PSETID_ADDRESS, 0x8208), None, "a name is its set and number together");
    assert_eq!(map.entries.len(), 4);
    assert!(NameMap::from_bag(&Bag::default()).entries.is_empty());
}

#[test]
fn the_meeting_set_is_spelled_as_files_store_it() {
    // {6ED8DA90-450B-101B-98DA-00AA003F1305}: the first three fields are little-endian.
    assert_eq!(&PSETID_MEETING[..4], &0x6ed8_da90u32.to_le_bytes());
    assert_eq!(&PSETID_MEETING[4..6], &0x450bu16.to_le_bytes());
    assert_eq!(&PSETID_MEETING[6..8], &0x101bu16.to_le_bytes());
    assert_eq!(&PSETID_APPOINTMENT[..4], &[0x02, 0x20, 0x06, 0x00]);
}
