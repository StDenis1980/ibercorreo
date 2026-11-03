use super::*;
use crate::testing::compress_rtf;

fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace().map(|h| u8::from_str_radix(h, 16).unwrap()).collect()
}

#[test]
fn the_specification_examples_decompress() {
    assert_eq!(PREFIX.len(), 207);
    // MS-OXRTFCP 3.1.1: literals and one dictionary reference.
    let one = hex(
        "2d 00 00 00 2b 00 00 00 4c 5a 46 75 f1 c5 c7 a7 03 00 0a 00 72 63 70 67 31 32 35 42 32 0a f3 20 68 65 6c 09 00 20 62 77 05 b0 6c 64 7d 0a 80 0f a0",
    );
    assert_eq!(String::from_utf8(decompress(&one).unwrap()).unwrap(), "{\\rtf1\\ansi\\ansicpg1252\\pard hello world}\r\n");
    // MS-OXRTFCP 3.1.2: a reference that reads what it is writing.
    let two = hex("1a 00 00 00 1c 00 00 00 4c 5a 46 75 e2 d4 4b 51 41 00 04 20 57 58 59 5a 0d 6e 7d 01 0e b0");
    assert_eq!(String::from_utf8(decompress(&two).unwrap()).unwrap(), "{\\rtf1 WXYZWXYZWXYZWXYZWXYZ}");
}

#[test]
fn long_bodies_wrap_the_dictionary() {
    let mut rtf = b"{\\rtf1\\ansi ".to_vec();
    for i in 0..900u32 {
        rtf.extend_from_slice(format!("line {i} of the body, {}\\par ", i * 7919 % 1013).as_bytes());
    }
    rtf.push(b'}');
    let packed = compress_rtf(&rtf);
    assert!(packed.len() < rtf.len() / 2, "references were used");
    assert_eq!(decompress(&packed).unwrap(), rtf);
}

#[test]
fn bad_streams_are_refused() {
    let good = compress_rtf(b"{\\rtf1 hello}");
    let mut corrupt = good.clone();
    let last = corrupt.len() - 3;
    corrupt[last] ^= 0x55;
    assert!(decompress(&corrupt).unwrap_err().to_string().contains("checksum"));
    assert!(decompress(&good[..10]).unwrap_err().to_string().contains("header"));
    let mut other = good.clone();
    other[8..12].copy_from_slice(b"ABCD");
    assert!(decompress(&other).unwrap_err().to_string().contains("compression type"));
    let mut huge = good.clone();
    huge[4..8].copy_from_slice(&(MAX_RTF_BYTES as u32 + 1).to_le_bytes());
    assert!(matches!(decompress(&huge), Err(Error::TooLarge { .. })));
    // Stored as is.
    let mut stored = Vec::new();
    stored.extend_from_slice(&(12u32 + 13).to_le_bytes());
    stored.extend_from_slice(&13u32.to_le_bytes());
    stored.extend_from_slice(b"MELA");
    stored.extend_from_slice(&0u32.to_le_bytes());
    stored.extend_from_slice(b"{\\rtf1 plain}trailing");
    assert_eq!(decompress(&stored).unwrap(), b"{\\rtf1 plain}");
    // A raw size smaller than the data stops the output there.
    let mut short = good.clone();
    short[4..8].copy_from_slice(&5u32.to_le_bytes());
    assert_eq!(decompress(&short).unwrap(), b"{\\rtf");
    // A raw size larger than the data: the end marker stops the output.
    let mut long = good.clone();
    long[4..8].copy_from_slice(&1000u32.to_le_bytes());
    assert_eq!(decompress(&long).unwrap(), b"{\\rtf1 hello}");
}

#[test]
fn hostile_streams_never_panic() {
    let good = compress_rtf(b"{\\rtf1\\ansi {\\*\\htmltag1 <p>}\\htmlrtf x\\htmlrtf0 hello \\u1078?}");
    for cut in 0..good.len() {
        let _ = decompress(&good[..cut]);
    }
    for at in 16..good.len() {
        for v in [0u8, 0xff, 0x0f, 0xf0] {
            let mut b = good.clone();
            b[at] = v;
            let sum = crc(&b[16..]);
            b[12..16].copy_from_slice(&sum.to_le_bytes());
            if let Ok(rtf) = decompress(&b) {
                let _ = body(&rtf);
            }
        }
    }
    for rtf in [
        &b"{\\rtf"[..],
        b"{\\rtf1 \\",
        b"{\\rtf1 \\'",
        b"{\\rtf1 \\'zz}",
        b"{\\rtf1 \\u-99999999999999999999?}",
        b"{\\rtf1 \\uc99999 \\u65 abc}",
        b"{\\rtf1 }}}}}",
        b"{\\rtf1 {\\*",
        b"{\\rtf1 \\ansicpg-5 \\'e9}",
        b"{\\rtf1 \\u-10179?}",
        b"{\\rtf1 \\u-8704?}",
    ] {
        let _ = body(rtf);
    }
    let mut deep = b"{\\rtf1 ".to_vec();
    deep.extend(std::iter::repeat_n(b'{', 100_000));
    deep.extend_from_slice(b"deep");
    deep.extend(std::iter::repeat_n(b'}', 100_000));
    deep.extend_from_slice(b" after}");
    assert_eq!(body(&deep), Some(Body::Text("deep after".into())), "groups past the cap still close in order");
    // Past the cap a group is read as part of its parent, so it cannot start a destination.
    let mut capped = b"{\\rtf1 ".to_vec();
    capped.extend(std::iter::repeat_n(b'{', MAX_GROUPS));
    capped.extend_from_slice(b"{\\fonttbl F;}");
    assert_eq!(body(&capped), Some(Body::Text("F;".into())));
}

#[test]
fn html_comes_back_out_of_its_rtf_wrapping() {
    let rtf = concat!(
        "{\\rtf1\\ansi\\ansicpg1251\\fromhtml1 \\deff0{\\fonttbl{\\f0\\fswiss\\fcharset204 Arial;}}",
        "{\\colortbl\\red0\\green0\\blue0;}\\uc1{\\*\\htmltag19 <html>}{\\*\\htmltag2 \\par }",
        "{\\*\\htmltag34 <body>}\\htmlrtf \\f0\\fs20 RTF only\\htmlrtf0 {\\*\\htmltag64 <p class=\"a\\{b\\}\">}",
        "\\htmlrtf {\\htmlrtf0 \\'cf\\'f0\\'e8\\'e2\\'b3\\'f2 &amp; caf\\u233?\\htmlrtf\\par\\htmlrtf0}",
        "{\\*\\htmltag72 </p>}{\\*\\mhtmltag84 <img src=\"cid:x\">}{\\*\\htmltag84 <img src=\"x\">}",
        "{\\*\\htmltag27 </body>}{\\*\\htmltag3 </html>}}\0\0garbage",
    );
    assert_eq!(body(rtf.as_bytes()), Some(Body::Html("<html>\r\n<body><p class=\"a{b}\">Привіт &amp; café</p><img src=\"x\"></body></html>".into())));
}

#[test]
fn text_comes_out_of_plain_and_rtf_native_bodies() {
    let from_text = b"{\\rtf1\\ansi\\ansicpg1252\\fromtext \\fbidis \\deff0{\\fonttbl{\\f0\\fmodern Courier New;}}\r\n\\uc1\\pard\\plain Hello\\par second line\\tab x\\par\r\n}";
    assert_eq!(body(from_text), Some(Body::Text("Hello\nsecond line\tx\n".into())));
    let native = concat!(
        "{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0 Arial;}}{\\*\\generator Riched20;}{\\info{\\author Me}}",
        "\\viewkind4\\pard\\b Bold\\b0  and {\\i italic}\\emdash\\~ok\\par \\uc2\\u-10179\\'3f?\\u-8704??\\'e9\\par}",
    );
    assert_eq!(body(native.as_bytes()), Some(Body::Text("Bold and italic\u{2014}\u{a0}ok\n😀é\n".into())));
    assert_eq!(body(b"plain words, no RTF"), None);
    assert_eq!(body(b"{\\rtf1\\fromhtml0 x}"), Some(Body::Text("x".into())));
    // A destination is named only at the head of its group.
    assert_eq!(body(b"{\\rtf1 a {\\b b \\info c\\*\\fonttbl d}}"), Some(Body::Text("a b cd".into())));
    assert_eq!(body(b"{not rtf}"), None);
}
