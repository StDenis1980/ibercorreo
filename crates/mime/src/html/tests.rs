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
fn text_inside_a_zero_font_size_layout_cell_shows() {
    // Email layout templates zero the font size on table cells and columns (so the spaces between
    // inline-block columns take no room) and set it again on the text inside.
    let r = to_rich(
        "<body><div style=\"display:none;font-size:1px;max-height:0px;opacity:0\">Preheader</div>\
         <table><tr><td style=\"font-size:0px;padding:0\">\
         <div style=\"font-size:0;display:inline-block\"><img src=\"cid:logo\" alt=\"Logo\"></div>\
         <div style=\"font-size:0px\"><div style=\"font-size:16px;line-height:1.5\">Payment <b>confirmed</b></div>\
         <span style=\"font-size: inherit\">gap</span> <br> </div>\
         </td></tr></table></body>",
    );
    assert_eq!(text(&r), ["Logo", "Payment confirmed"], "the image shows, the inherited zero size hides \"gap\"");
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

/// Tinta: a table of data keeps its rows and cells; tables that only lay a message out do not.
#[test]
fn tables_of_data_keep_their_cells_and_layout_tables_flow() {
    let r = to_rich(
        "<p>Your order</p><table border=1><tr><th>Item</th><th>Price</th></tr>\
         <tr><td>Tea</td><td>3 €</td></tr><tr><td><p>Cake</p><p>(slice)</p></td><td>4 €</td></tr></table><p>Thanks</p>",
    );
    let cells: Vec<_> = r.blocks.iter().map(|b| b.cell.map(|c| (c.row, c.col, c.header))).collect();
    assert_eq!(
        cells,
        [
            None,
            Some((0, 0, true)),
            Some((0, 1, true)),
            Some((1, 0, false)),
            Some((1, 1, false)),
            Some((2, 0, false)),
            Some((2, 0, false)),
            Some((2, 1, false)),
            None
        ]
    );
    assert_eq!(r.to_text(), "Your order\n\nItem\tPrice\nTea\t3 €\nCake\n(slice)\t4 €\n\nThanks");

    // A one-column layout, a table that holds another, and a single row are no tables of data.
    let layout = to_rich("<table><tr><td>Logo</td></tr><tr><td>Hello</td></tr></table>");
    assert!(layout.blocks.iter().all(|b| b.cell.is_none()));
    let row = to_rich("<table><tr><td>Left</td><td>Right</td></tr></table>");
    assert!(row.blocks.iter().all(|b| b.cell.is_none()));
    let nested = to_rich(
        "<table><tr><td>Head</td><td>x</td></tr><tr><td>\
         <table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>\
         </td><td>y</td></tr></table>",
    );
    let kept: Vec<_> =
        nested.blocks.iter().filter_map(|b| b.cell.map(|c| (b.spans.first().map(|s| s.text.clone()).unwrap_or_default(), c.table))).collect();
    let inner = kept.first().map(|k| k.1);
    assert_eq!(kept.iter().map(|k| k.0.as_str()).collect::<Vec<_>>(), ["a", "b", "c", "d"], "only the inner table holds data");
    assert!(kept.iter().all(|k| Some(k.1) == inner));

    // Unclosed and stray tags end the table as browsers would, and nothing panics.
    for html in ["<table><tr><td>a<td>b<tr><td>c<td>d", "</td></tr><td>x</table><table>", "<div><table><tr><td>a<td>b</div><tr><td>c<td>d"] {
        let r = to_rich(html);
        assert!(r.blocks.len() <= 4, "{html}");
    }
    let wide: String = format!("<table><tr>{}</tr><tr>{}</tr></table>", "<td>a</td>".repeat(40), "<td>b</td>".repeat(40));
    assert!(to_rich(&wide).blocks.iter().all(|b| b.cell.is_none()), "too wide to be a table");
}

/// Tinta: colours, highlights and clearly different sizes are kept; near-normal sizes and the
/// backgrounds of blocks are not.
#[test]
fn colours_and_sizes_are_kept() {
    let r = to_rich(
        "<p><span style=\"color:#c00\">red</span> <font color=\"navy\">navy</font> \
         <span style=\"color: rgb(0, 128, 0)\">green</span> <mark>marked</mark> \
         <span style=\"background-color:#ffff00\">yellow</span> \
         <span style=\"font-size:30px\">big</span> <span style=\"font-size:14px\">normal</span> \
         <font size=\"1\">tiny</font> <span style=\"font-family: 'Courier New', monospace\">code</span></p>\
         <div style=\"background:#ffffff;color:#333333\">plain</div>",
    );
    let span = |t: &str| r.blocks.iter().flat_map(|b| &b.spans).find(|s| s.text.trim() == t).cloned().unwrap_or_default();
    assert_eq!(span("red").color, Some(0xcc0000));
    assert_eq!(span("navy").color, Some(0x000080));
    assert_eq!(span("green").color, Some(0x008000));
    assert!(span("marked").highlight.is_some());
    assert_eq!(span("yellow").highlight, Some(0xffff00));
    assert_eq!(span("big").size, Some(200));
    assert_eq!(span("normal").size, None);
    assert_eq!(span("tiny").size, Some(60));
    assert!(span("code").code);
    assert_eq!(span("plain").highlight, None, "a block's background is not a highlight");
    assert_eq!(span("plain").color, Some(0x333333));

    for bad in ["#12", "rgb(1,2)", "rgba(0,0,0,0)", "transparent", "url(x)", "#zzzzzz", "rgb(nan, 1e99, -5)"] {
        let r = to_rich(&format!("<span style=\"color:{bad};font-size:{bad}\">x</span>"));
        let s = r.blocks.first().and_then(|b| b.spans.first()).cloned().unwrap_or_default();
        assert!(s.size.is_none(), "{bad}");
        if bad != "rgb(nan, 1e99, -5)" {
            assert!(s.color.is_none(), "{bad}");
        }
    }
    assert_eq!(css_color("rgb(nan, 1e99, -5)"), Some(0x00ff00), "clamped, never a panic");
}
