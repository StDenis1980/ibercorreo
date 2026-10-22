use super::*;

fn text(r: &Rich) -> Vec<String> {
    r.blocks.iter().map(|b| b.spans.iter().map(|s| s.text.as_str()).collect()).collect()
}

#[test]
fn structure_and_inline_style() {
    let r = to_rich(
        "<html><head><title>T</title><style>p{color:red}</style></head><body>\
         <h1>Agenda</h1><p>Hello <b>Olena</b>, see <i>this</i>.</p>\
         <ul><li>One</li><li>Two<ol start=\"3\"><li>Three</li><li>Four</ol></ul>\
         <blockquote><p>Earlier</p><blockquote>Older</blockquote></blockquote><hr><pre>  a\n    b</pre></body></html>",
    );
    assert_eq!(text(&r), ["Agenda", "Hello Olena, see this.", "One", "Two", "Three", "Four", "Earlier", "Older", "", "  a\n    b"]);
    let kinds: Vec<_> = r.blocks.iter().map(|b| (b.kind, b.quote)).collect();
    assert_eq!(
        kinds,
        [
            (BlockKind::Heading(1), 0),
            (BlockKind::Paragraph, 0),
            (BlockKind::Bullet { depth: 1 }, 0),
            (BlockKind::Bullet { depth: 1 }, 0),
            (BlockKind::Numbered { depth: 2, number: 3 }, 0),
            (BlockKind::Numbered { depth: 2, number: 4 }, 0),
            (BlockKind::Paragraph, 1),
            (BlockKind::Paragraph, 2),
            (BlockKind::Rule, 0),
            (BlockKind::Pre, 0),
        ]
    );
    let p = &r.blocks[1].spans;
    assert!(p[1].bold && p[1].text == "Olena" && !p[0].bold);
    assert!(p[3].italic);
}

#[test]
fn nothing_active_gets_through() {
    let r = to_rich(
        "<p onclick=\"steal()\">Hi<script>alert(1)</script><SCRIPT type=x>alert(2)</ScRiPt>\
         <iframe src=\"https://evil.example\">frame text</iframe><svg><text>svg text</text></svg>\
         <form action=\"https://evil.example\"><input value=\"pw\"><select><option>opt</option></select></form>\
         <noscript>ns</noscript><template>tpl</template>there</p>",
    );
    assert_eq!(text(&r), ["Hi", "there"], "a form is a block of its own");
    for href in [
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        "java\nscript:alert(1)",
        " javascript:alert(1)",
        "&#106;avascript:alert(1)",
        "vbscript:x",
        "data:text/html,<script>",
        "file:///etc/passwd",
        "/relative",
        "#top",
    ] {
        let r = to_rich(&format!("<a href=\"{href}\">x</a>"));
        assert_eq!(r.blocks[0].spans[0].link, None, "{href}");
    }
    let r = to_rich("<a href=\"https://ok.example/a?b=1&amp;c=2\">ok</a> <a href='mailto:a@b.example'>m</a>");
    let links: Vec<_> = r.blocks[0].spans.iter().filter_map(|s| s.link.as_deref()).collect();
    assert_eq!(links, ["https://ok.example/a?b=1&c=2", "mailto:a@b.example"]);
}

#[test]
fn hidden_text_and_trackers_are_dropped() {
    let r = to_rich(
        "<div style=\"DISPLAY: none !important\">Preview text you never see&zwnj;&nbsp;&zwnj;&nbsp;</div>\
         <div style=\"max-height:0;overflow:hidden\">more preview</div>\
         <span style=\"font-size:0px\">hidden</span><p hidden>also hidden</p>\
         <p>Visible<img src=\"https://t.example/open.gif\" width=\"1\" height=\"1\"></p>\
         <p><img src=\"https://cdn.example/logo.png\" alt=\"Shop logo\"> <img src=\"cid:part1\"></p>",
    );
    assert_eq!(text(&r), ["Visible", "Shop logo image"]);
    assert_eq!(r.remote_images, 2, "the pixel and the logo were both remote");
    assert!(r.blocks[1].spans.iter().filter(|s| s.image).count() == 2);
}

#[test]
fn phishing_shaped_links_are_marked() {
    let r = to_rich(
        "<a href=\"https://login.evil.example/\">https://www.bank.example/login</a> \
         <a href=\"https://www.bank.example/x\">bank.example</a> \
         <a href=\"https://mail.bank.example/\">www.bank.example</a> \
         <a href=\"https://evil.example/\">Your account</a>",
    );
    let marks: Vec<_> = r.blocks[0].spans.iter().filter(|s| s.link.is_some()).map(|s| (s.text.as_str(), s.deceptive)).collect();
    assert_eq!(marks, [("https://www.bank.example/login", true), ("bank.example", false), ("www.bank.example", false), ("Your account", false)]);
}

#[test]
fn entities_and_whitespace() {
    assert_eq!(
        decode_entities("a &amp; b &lt;c&gt; &eacute;&Ccedil;&atilde; &#233;&#x1F600; &#150; &nbsp &bogus; &"),
        "a & b <c> éÇã é😀 – \u{A0} &bogus; &"
    );
    assert_eq!(decode_entities("&#0;&#99999999999;&#xD800;"), "\u{FFFD}\u{FFFD}\u{FFFD}");
    let r = to_rich("<p>  one\n\t two  </p><p>three<br>  four<br><br></p>");
    assert_eq!(text(&r), ["one two", "three\nfour"]);
}

#[test]
fn sloppy_html_still_reads() {
    let r = to_rich("<p>one<p>two<li>three<li>four</div></span><td>five<td>six<tr><td>seven");
    assert_eq!(text(&r), ["one", "two", "three", "four", "five", "six", "seven"]);
    let r = to_rich("a < b and <3 <!-- note --> c <!doctype html><?xml x?> d </ x>");
    assert_eq!(text(&r), ["a < b and <3 c d"]);
    let r = to_rich("<a href=\"https://x.example\" title='unclosed>text");
    assert!(r.blocks.is_empty());
    let r = to_rich("<p>open <b>bold <i>both</p><p>after</p>");
    assert_eq!(text(&r), ["open bold both", "after"]);
    assert!(!r.blocks[1].spans[0].bold, "styles end with their block's elements");
}

#[test]
fn hostile_input_is_bounded() {
    let deep = "<div><blockquote><ul>".repeat(5_000) + "deep" + &"</ul></blockquote></div>".repeat(5_000);
    let r = to_rich(&deep);
    assert_eq!(text(&r), ["deep"]);
    assert!(r.blocks[0].quote <= 8);
    let many = "<p>x</p>".repeat(MAX_BLOCKS + 10);
    assert_eq!(to_rich(&many).blocks.len(), MAX_BLOCKS);
    for raw in [
        "<",
        "&",
        "&#",
        "&#x",
        "<a",
        "<a href",
        "<a href=",
        "<a href='",
        "</",
        "<!--",
        "<!",
        "<script>",
        "<script>never ends",
        "<p a=\u{301}b>é</p>",
        "\u{0}<\u{0}p\u{0}>",
        "<img src=x alt=\"&#xFFFFFF;\">",
        "<ol start=99999999999><li>x",
        "<li value=4294967295>a<li>b",
    ] {
        let _ = to_rich(raw);
    }
    let r = to_rich("<ol start=4294967295><li>a<li>b</ol>");
    assert_eq!(
        r.blocks.iter().map(|b| b.kind).collect::<Vec<_>>(),
        [BlockKind::Numbered { depth: 1, number: u32::MAX }, BlockKind::Numbered { depth: 1, number: u32::MAX }]
    );
}
