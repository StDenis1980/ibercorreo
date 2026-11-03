use super::*;
use crate::testing::{Node, build, storage, stream};

fn sample() -> Vec<Node> {
    vec![
        stream("small", b"hello".to_vec()),
        storage(
            "box",
            vec![stream("inner", vec![7u8; 100]), storage("deeper", vec![stream("big", (0..5000u32).map(|i| i as u8).collect::<Vec<u8>>())])],
        ),
        stream("empty", Vec::new()),
    ]
}

fn check(c: &Cfb<'_>) {
    let small = c.find(0, "SMALL").unwrap();
    assert_eq!(c.read(small).unwrap(), b"hello", "names match in any case; a mini stream");
    let b = c.find(0, "box").unwrap();
    assert_eq!(c.entry(b).unwrap().kind, Kind::Storage);
    assert_eq!(c.read(c.find(b, "inner").unwrap()).unwrap(), vec![7u8; 100]);
    let deeper = c.find(b, "deeper").unwrap();
    let big = c.read(c.find(deeper, "big").unwrap()).unwrap();
    assert_eq!(big, (0..5000u32).map(|i| i as u8).collect::<Vec<u8>>(), "a regular stream");
    assert_eq!(c.read(c.find(0, "empty").unwrap()).unwrap(), Vec::<u8>::new());
    assert_eq!(c.children(0).len(), 3);
    assert!(c.find(0, "inner").is_none(), "only direct children");
    assert!(c.read(b).is_err(), "a storage is not a stream");
}

#[test]
fn streams_are_read_from_both_sector_sizes() {
    for shift in [9, 12] {
        let bytes = build(sample(), shift);
        check(&Cfb::open(&bytes).unwrap());
    }
}

#[test]
fn a_large_file_reads_its_fat_through_difat_sectors() {
    // Over 109 FAT sectors of 512 bytes: more than 109 * 128 sectors.
    let data: Vec<u8> = (0..7_400_000u32).map(|i| (i % 251) as u8).collect();
    let bytes = build(vec![stream("data", data.clone())], 9);
    assert!(u32_at(&bytes, 0x48).unwrap() > 0, "the file needs DIFAT sectors");
    let c = Cfb::open(&bytes).unwrap();
    assert_eq!(c.read(c.find(0, "data").unwrap()).unwrap(), data);
    // A DIFAT sector that points back at itself.
    let first = u32_at(&bytes, 0x44).unwrap() as usize;
    let mut looped = bytes.clone();
    let at = (first + 1) * 512 + 508;
    looped[at..at + 4].copy_from_slice(&(first as u32).to_le_bytes());
    assert!(Cfb::open(&looped).err().unwrap().to_string().contains("DIFAT chain loops"));
}

#[test]
fn hostile_files_are_refused_without_panicking() {
    let good = build(sample(), 9);
    assert!(Cfb::open(b"not a compound file at all").is_err());
    for cut in (0..good.len()).step_by(7) {
        if let Ok(c) = Cfb::open(&good[..cut]) {
            for i in 0..20 {
                let _ = c.read(i);
                let _ = c.children(i);
            }
        }
    }
    // Every u32 in the header and the directory set to values that point anywhere.
    for at in (0..good.len().min(4096)).step_by(4) {
        for v in [0u32, 1, 2, 0xffff_fffe, 0xffff_fffa, 0x7fff_ffff, 3] {
            let mut b = good.clone();
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
            if let Ok(c) = Cfb::open(&b) {
                for i in 0..20 {
                    let _ = c.read(i);
                    let _ = c.find(i, "x");
                }
            }
        }
    }
    // A FAT chain that loops on itself.
    let mut looped = good.clone();
    let fat = 512;
    let dir_start = u32_at(&good, 0x30).unwrap() as usize;
    looped[fat + dir_start * 4..fat + dir_start * 4 + 4].copy_from_slice(&(dir_start as u32).to_le_bytes());
    assert!(Cfb::open(&looped).is_err(), "a looping directory chain");
    // A directory whose tree loops back to the root.
    let mut tree = good.clone();
    let dir = 512 * (dir_start + 1);
    tree[dir + 128 + 72..dir + 128 + 76].copy_from_slice(&1u32.to_le_bytes());
    let c = Cfb::open(&tree).unwrap();
    assert!(c.children(0).len() <= 8);
    // A stream claiming to be larger than the file.
    let mut huge = good.clone();
    huge[dir + 128 * 5 + 120..dir + 128 * 5 + 124].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
    let c = Cfb::open(&huge).unwrap();
    assert!((0..8).any(|i| c.read(i).is_err()));
    assert!(matches!(Cfb::open(&vec![0; MAX_FILE + 1]), Err(Error::TooLarge { .. })));
}

/// The byte offset of directory entry `n` in a file built by [`build`] with 512-byte sectors.
fn entry_at(file: &[u8], n: usize) -> usize {
    512 * (u32_at(file, 0x30).unwrap() as usize + 1) + 128 * n
}

/// Entry numbers in `sample()`: 0 root, 1 small, 2 box, 3 inner, 4 deeper, 5 big, 6 empty.
#[test]
fn inconsistent_files_are_refused() {
    let good = build(sample(), 9);
    let err = |b: &[u8]| Cfb::open(b).err().map(|e| e.to_string()).unwrap_or_default();
    // The header names the same FAT sector 109 times in a file of a few sectors.
    let mut many = good.clone();
    for i in 0..HEADER_DIFAT {
        many[0x4c + i * 4..0x50 + i * 4].copy_from_slice(&0u32.to_le_bytes());
    }
    assert!(err(&many).contains("more FAT sectors than sectors"), "{}", err(&many));
    // The first entry is not the root.
    let mut rootless = good.clone();
    let root = entry_at(&good, 0);
    rootless[root + 66] = 1;
    assert!(err(&rootless).contains("no root entry"));

    let read_err = |b: &[u8], n: usize| Cfb::open(b).unwrap().read(n).err().map(|e| e.to_string()).unwrap_or_default();
    // A regular stream larger than the file, and one larger than its chain.
    let big = entry_at(&good, 5);
    let mut huge = good.clone();
    huge[big + 120..big + 124].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
    assert!(read_err(&huge, 5).contains("larger than the file"));
    let mut long = good.clone();
    long[big + 120..big + 124].copy_from_slice(&5600u32.to_le_bytes());
    assert!(read_err(&long, 5).contains("shorter than its size"));
    // A small stream larger than the mini stream, and a mini chain that loops.
    let inner = entry_at(&good, 3);
    let mut mini_huge = good.clone();
    mini_huge[inner + 120..inner + 124].copy_from_slice(&4000u32.to_le_bytes());
    assert!(read_err(&mini_huge, 3).contains("larger than the mini stream"));
    let start = u32_at(&good, inner + 116).unwrap() as usize;
    let mini_fat = 512 * (u32_at(&good, 0x3c).unwrap() as usize + 1);
    let mut mini_loop = good.clone();
    mini_loop[mini_fat + start * 4..mini_fat + start * 4 + 4].copy_from_slice(&(start as u32).to_le_bytes());
    assert!(read_err(&mini_loop, 3).contains("loops"), "{}", read_err(&mini_loop, 3));
    // A free slot is neither a storage nor a stream.
    let mut free = good.clone();
    free[entry_at(&good, 2) + 66] = 0;
    let c = Cfb::open(&free).unwrap();
    assert_eq!(c.entry(2).unwrap().kind, Kind::Unused);
}
