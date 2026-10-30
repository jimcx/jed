use tree_sitter::{Node, Query};

use super::*;
use crate::buffer::Buffer;
use crate::buffer::change::{Change, ChangeSet};
use crate::buffer::undo::EditKind;
use crate::selection::{Selection, Selections};

const MAX: usize = DEFAULT_MAX_BYTES;

fn fresh(language: Language, text: &str) -> Tree {
    new_parser(language).parse(text, None).unwrap()
}

/// A buffer in `language` with its first parse done (run here instead of off-thread).
fn parsed_buffer(language: Language, text: &str) -> Buffer {
    let mut buf = Buffer::from_str(text);
    buf.syntax.set_language(language);
    assert!(buf.syntax.tree().is_none());
    let job = buf.syntax.update(&buf.rope, MAX).expect("a first parse");
    assert!(buf.syntax.update(&buf.rope, MAX).is_none(), "one parse at a time");
    buf.syntax.parsed(job.run());
    assert!(buf.syntax.update(&buf.rope, MAX).is_none());
    buf
}

/// Brings the tree up to date as the app does: inline, or through background parses.
fn settle(buf: &mut Buffer) {
    while let Some(job) = buf.syntax.update(&buf.rope, MAX) {
        buf.syntax.parsed(job.run());
    }
    assert!(buf.syntax.tree().is_some());
}

/// Every node's kind, byte range and positions, depth first.
fn nodes(tree: &Tree) -> Vec<String> {
    fn walk(n: Node, out: &mut Vec<String>) {
        out.push(format!(
            "{} {:?} {:?}-{:?}",
            n.kind(),
            n.byte_range(),
            n.start_position(),
            n.end_position()
        ));
        let mut c = n.walk();
        for child in n.children(&mut c) {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    walk(tree.root_node(), &mut out);
    out
}

fn assert_matches_fresh(buf: &Buffer, context: &str) {
    let text = buf.rope.to_string();
    let tree = buf.syntax.tree().expect("tree");
    let want = fresh(buf.syntax.language(), &text);
    assert_eq!(
        tree.root_node().to_sexp(),
        want.root_node().to_sexp(),
        "{context}\n{text}"
    );
    assert_eq!(nodes(tree), nodes(&want), "{context}\n{text}");
}

fn edit(buf: &mut Buffer, changes: Vec<Change>) {
    let sel = Selections::single(Selection::cursor(ByteIdx(0)));
    buf.edit(ChangeSet::new(changes), sel.clone(), sel, EditKind::Other);
}

fn change(start: usize, end: usize, text: &str) -> Change {
    Change {
        start: ByteIdx(start),
        end: ByteIdx(end),
        text: text.into(),
    }
}

#[test]
fn grammars_load_and_queries_compile() {
    for l in Language::BUILT_IN {
        let Some(grammar) = l.grammar() else {
            assert!(l.highlight_queries().is_empty());
            continue;
        };
        assert!(
            (tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION..=tree_sitter::LANGUAGE_VERSION)
                .contains(&grammar.abi_version()),
            "{l:?}"
        );
        let source = l.highlight_queries().join("\n");
        let query = Query::new(&grammar, &source).unwrap_or_else(|e| panic!("{l:?}: {e}"));
        assert!(query.capture_names().len() > 5, "{l:?}");
    }
}

#[test]
fn parses_small_snippets() {
    let c = fresh(Language::C, "int main(void) { return 0; }\n");
    assert_eq!(
        c.root_node().to_sexp(),
        "(translation_unit (function_definition type: (primitive_type) declarator: (function_declarator \
         declarator: (identifier) parameters: (parameter_list (parameter_declaration type: (primitive_type)))) \
         body: (compound_statement (return_statement (number_literal)))))"
    );
    let cpp = fresh(Language::Cpp, "namespace a { class B {}; }\n");
    assert_eq!(
        cpp.root_node().to_sexp(),
        "(translation_unit (namespace_definition name: (namespace_identifier) body: (declaration_list \
         (class_specifier name: (type_identifier) body: (field_declaration_list)))))"
    );
    let py = fresh(Language::Python, "def f(x):\n    return x\n");
    assert_eq!(
        py.root_node().to_sexp(),
        "(module (function_definition name: (identifier) parameters: (parameters (identifier)) \
         body: (block (return_statement (identifier)))))"
    );
}

#[test]
fn reads_the_rope_across_chunks() {
    // Big enough for many rope chunks, with multibyte text at chunk ends.
    let text = "int f界(void) { return 1; } // ü\n".repeat(2000);
    let buf = parsed_buffer(Language::Cpp, &text);
    assert_matches_fresh(&buf, "chunks");
    assert!(!buf.syntax.tree().unwrap().root_node().has_error());
}

#[test]
fn plain_text_has_no_tree() {
    let mut buf = Buffer::from_str("hello\n");
    buf.syntax.set_language(Language::PlainText);
    assert!(buf.syntax.update(&buf.rope, MAX).is_none());
    assert!(buf.syntax.tree().is_none());
    assert!(!buf.syntax.wants_edits());
}

#[test]
fn edits_and_undo_keep_the_tree_current() {
    let mut buf = parsed_buffer(Language::Cpp, "int a = 1;\nint b = 2;\n");
    // Two changes in one set: positions of the second shift by the first.
    edit(&mut buf, vec![change(4, 5, "alpha"), change(15, 16, "beta\n")]);
    assert!(buf.syntax.tree().is_none(), "edited, not re-parsed yet");
    settle(&mut buf);
    assert_eq!(buf.rope.to_string(), "int alpha = 1;\nint beta\n = 2;\n");
    assert_matches_fresh(&buf, "edit");
    buf.undo();
    settle(&mut buf);
    assert_matches_fresh(&buf, "undo");
    buf.redo();
    settle(&mut buf);
    assert_matches_fresh(&buf, "redo");
}

/// Seeded xorshift: `next(n)` is in `0..n`.
struct Rng(u64);
impl Rng {
    fn next(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// A random sorted change set on char boundaries of `text`.
fn random_changes(rng: &mut Rng, text: &str, n: usize) -> Vec<Change> {
    const PIECES: &[&str] = &[
        "x",
        "int ",
        ";",
        "{",
        "}",
        "(",
        ")",
        "\n",
        "\r\n",
        "\r",
        " ",
        "界",
        "ü",
        "\"s\"",
        "// c\n",
        "/*",
        "*/",
        "0x1f",
        "\t",
        "::",
        "#define X 1\n",
        "",
    ];
    let bounds: Vec<usize> = (0..=text.len()).filter(|&i| text.is_char_boundary(i)).collect();
    let mut points: Vec<usize> = (0..n * 2).map(|_| bounds[rng.next(bounds.len())]).collect();
    points.sort_unstable();
    points
        .chunks(2)
        .map(|p| {
            // Mostly short deletions, so the text doesn't shrink away.
            let end = if rng.next(3) == 0 { p[1].min(p[0] + 8) } else { p[0] };
            let end = bounds[bounds.partition_point(|&b| b < end)];
            change(p[0], end, PIECES[rng.next(PIECES.len())])
        })
        .collect::<Vec<_>>()
        .into_iter()
        .scan(0, |last_end, c| {
            // Keep them non-overlapping.
            let c = if c.start.0 < *last_end {
                change(*last_end, (*last_end).max(c.end.0), &c.text)
            } else {
                c
            };
            *last_end = c.end.0;
            Some(c)
        })
        .collect()
}

#[test]
fn random_edits_match_a_fresh_parse() {
    let seed_text = "#include <x>\nint main(int argc, char **argv) {\n  if (argc > 1) { return f(\"界\"); }\n  \
                     // comment ü\n  return 0;\n}\nstruct S { int a; };\n";
    for (language, seed) in [(Language::Cpp, 1u64), (Language::C, 7), (Language::Python, 42)] {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut buf = parsed_buffer(language, &seed_text.repeat(20));
        for step in 0..60 {
            let r = rng.next(10);
            if r == 0 {
                buf.undo();
            } else if r == 1 {
                buf.redo();
            } else {
                // Sometimes a set big enough to be coalesced into one `InputEdit`.
                let n = if r == 2 { COALESCE_AT + 50 } else { 1 + rng.next(4) };
                let text = buf.rope.to_string();
                edit(&mut buf, random_changes(&mut rng, &text, n));
            }
            // Several edits before a re-parse, sometimes.
            if rng.next(3) != 0 {
                settle(&mut buf);
                assert_matches_fresh(&buf, &format!("{language:?} step {step}"));
            }
        }
        settle(&mut buf);
        assert_matches_fresh(&buf, &format!("{language:?} end"));
    }
}

#[test]
fn edits_during_the_first_parse_are_applied_to_its_result() {
    let mut buf = Buffer::from_str("int a;\nint b;\n");
    buf.syntax.set_language(Language::C);
    let job = buf.syntax.update(&buf.rope, MAX).unwrap();
    // Edited while the parse "runs" off-thread.
    edit(&mut buf, vec![change(0, 0, "long x;\n"), change(11, 12, "bee")]);
    buf.undo();
    buf.redo();
    edit(&mut buf, vec![change(0, 4, "")]);
    assert!(buf.syntax.update(&buf.rope, MAX).is_none(), "still parsing");
    buf.syntax.parsed(job.run());
    assert!(buf.syntax.tree().is_none(), "the result is behind the text");
    settle(&mut buf);
    assert_matches_fresh(&buf, "stale first parse");
}

#[test]
fn superseded_or_cancelled_parses_are_dropped() {
    let mut buf = Buffer::from_str("int a;\n");
    buf.syntax.set_language(Language::C);
    let old = buf.syntax.update(&buf.rope, MAX).unwrap();
    // A language change drops the running parse (and cancels it).
    buf.syntax.set_language(Language::Cpp);
    assert!(old.cancel.load(Ordering::Relaxed));
    let old = old.run();
    assert!(old.tree.is_none());
    let job = buf.syntax.update(&buf.rope, MAX).unwrap();
    buf.syntax.parsed(old);
    assert!(buf.syntax.tree().is_none());
    buf.syntax.parsed(job.run());
    assert!(buf.syntax.update(&buf.rope, MAX).is_none());
    assert_matches_fresh(&buf, "cpp");
}

#[test]
fn big_edits_parse_in_the_background() {
    let mut buf = parsed_buffer(Language::C, "int a;\n");
    let big = "int x;\n".repeat(INLINE_BYTES / 7 + 1);
    edit(&mut buf, vec![change(0, 0, &big)]);
    let job = buf.syntax.update(&buf.rope, MAX).expect("off-thread");
    assert!(job.old_tree.is_some(), "incremental");
    assert!(buf.syntax.tree().is_none());
    buf.syntax.parsed(job.run());
    assert!(buf.syntax.update(&buf.rope, MAX).is_none());
    assert_matches_fresh(&buf, "big");
}

#[test]
fn no_tree_past_the_size_limit() {
    let mut buf = Buffer::from_str("int a;\n");
    buf.syntax.set_language(Language::C);
    assert!(buf.syntax.update(&buf.rope, 4).is_none());
    assert!(buf.syntax.too_large());
    assert!(buf.syntax.tree().is_none());
    assert!(!buf.syntax.wants_edits());
    // Back under the limit: parsed again.
    let job = buf.syntax.update(&buf.rope, MAX).unwrap();
    buf.syntax.parsed(job.run());
    assert!(buf.syntax.update(&buf.rope, MAX).is_none());
    assert_matches_fresh(&buf, "under");
    // Growing past it drops the tree.
    edit(&mut buf, vec![change(0, 0, "int b;\n")]);
    assert!(buf.syntax.update(&buf.rope, 8).is_none());
    assert!(buf.syntax.too_large());
}

#[test]
fn ts_points_count_lf_only() {
    let rope = Rope::from_str("a\r\nb\rc\nd");
    let p = |i| pos::ts_point(&rope, ByteIdx(i));
    assert_eq!(p(0), Point::new(0, 0));
    assert_eq!(p(3), Point::new(1, 0));
    assert_eq!(p(6), Point::new(1, 3), "a lone CR isn't a row break");
    assert_eq!(p(7), Point::new(2, 0));
    assert_eq!(p(8), Point::new(2, 1));
}

#[test]
fn slow_reparses_continue_in_the_background() {
    // Opening a comment at the top re-parses all of it: far more than `INLINE_TIME`.
    let mut buf = parsed_buffer(Language::C, &"int f(void) { return 1; }\n".repeat(20_000));
    edit(&mut buf, vec![change(0, 0, "/*")]);
    let job = buf.syntax.update(&buf.rope, MAX).expect("gave up on the UI thread");
    assert!(job.old_tree.is_some());
    assert!(buf.syntax.tree().is_none());
    buf.syntax.parsed(job.run());
    assert!(buf.syntax.update(&buf.rope, MAX).is_none());
    assert_matches_fresh(&buf, "comment");
}

/// tree-sitter's point of byte `i` in `s`, counted independently of `pos::ts_point`.
fn point_in(s: &str, i: usize) -> Point {
    let before = &s.as_bytes()[..i];
    let row = before.iter().filter(|&&b| b == b'\n').count();
    let line_start = before.iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
    Point::new(row, i - line_start)
}

/// The `InputEdit`s for `changes` on `text`, computed by applying them one at a time to a string.
fn reference_edits(text: &str, changes: &[Change]) -> Vec<InputEdit> {
    let mut s = text.to_string();
    let mut delta: isize = 0;
    let mut out = Vec::new();
    for c in changes {
        let at = (c.start.0 as isize + delta) as usize;
        let old_end = at + (c.end.0 - c.start.0);
        let (start_position, old_end_position) = (point_in(&s, at), point_in(&s, old_end));
        s.replace_range(at..old_end, &c.text);
        out.push(InputEdit {
            start_byte: at,
            old_end_byte: old_end,
            new_end_byte: at + c.text.len(),
            start_position,
            old_end_position,
            new_end_position: point_in(&s, at + c.text.len()),
        });
        delta += c.text.len() as isize - (c.end.0 - c.start.0) as isize;
    }
    out
}

#[test]
fn input_edits_match_a_reference() {
    // CRLF, lone CR and multibyte text: rows count LFs only, columns are bytes.
    let text = "a\r\nbü\rc\n界d\r\n\re\n".repeat(40);
    let mut rng = Rng(0x5eed);
    for round in 0..200 {
        let n = if round % 10 == 0 {
            COALESCE_AT + 20
        } else {
            1 + rng.next(8)
        };
        let changes = random_changes(&mut rng, &text, n);
        let set = ChangeSet::new(changes.clone());
        let old = Rope::from_str(&text);
        let mut new = old.clone();
        set.apply(&mut new);
        let mut got = Vec::new();
        input_edits(&set, &old, &new, |e| got.push(e));
        // `ChangeSet::new` drops empty changes.
        let kept: Vec<Change> = changes
            .into_iter()
            .filter(|c| c.start != c.end || !c.text.is_empty())
            .collect();
        let want = if kept.len() > COALESCE_AT {
            let (first, last) = (kept[0].start.0, kept.last().unwrap().end.0);
            let new_end = (last as isize + new.len() as isize - old.len() as isize) as usize;
            let new_text = new.to_string();
            vec![InputEdit {
                start_byte: first,
                old_end_byte: last,
                new_end_byte: new_end,
                start_position: point_in(&text, first),
                old_end_position: point_in(&text, last),
                new_end_position: point_in(&new_text, new_end),
            }]
        } else {
            reference_edits(&text, &kept)
        };
        assert_eq!(got, want, "round {round}");
    }
}

#[test]
fn wide_change_sets_start_over() {
    // Hundreds of cursors spread over more than `INLINE_BYTES`: no tree edit (it would visit every
    // node on the UI thread), a fresh background parse instead.
    let text = "int f(void) { return 1; }\n".repeat(INLINE_BYTES / 20);
    let lines = text.len() / 26;
    let every_line = |text: &str| -> Vec<Change> {
        (0..COALESCE_AT + 1)
            .map(|i| i * (lines / (COALESCE_AT + 1)) * 26)
            .filter(|&p| p < text.len())
            .map(|p| change(p, p + 3, "long"))
            .collect()
    };
    let mut buf = parsed_buffer(Language::C, &text);
    edit(&mut buf, every_line(&text));
    assert!(buf.syntax.tree().is_none());
    assert!(
        buf.syntax.discard.is_some(),
        "the old tree waits to be freed off-thread"
    );
    let job = buf.syntax.update(&buf.rope, MAX).expect("a background parse");
    assert!(job.old_tree.is_none(), "a fresh parse");
    assert!(job.discard.is_some() && buf.syntax.discard.is_none());
    // The same again while that parse runs: it is cancelled and a new one starts.
    let text = buf.rope.to_string();
    edit(&mut buf, every_line(&text));
    assert!(job.cancel.load(Ordering::Relaxed));
    let stale = job.run();
    assert!(stale.tree.is_none());
    buf.syntax.parsed(stale);
    settle(&mut buf);
    assert_matches_fresh(&buf, "wide");
}

#[test]
fn stopped_parses_are_dropped() {
    let rope = Rope::from_str(&"int f(void) { return 1; }\n".repeat(100));
    let mut parser = new_parser(Language::C);
    assert!(parse(&mut parser, &rope, None, Stop::After(Instant::now())).is_none());
    assert!(parse(&mut parser, &rope, None, Stop::Cancel(&AtomicBool::new(true))).is_none());
    // The parser is reset, not resumed.
    let tree = parse(&mut parser, &rope, None, Stop::Cancel(&AtomicBool::new(false))).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        fresh(Language::C, &rope.to_string()).root_node().to_sexp()
    );
}

#[test]
fn size_limit_boundary() {
    let text = "int a;\n";
    let mut buf = Buffer::from_str(text);
    buf.syntax.set_language(Language::C);
    // Exactly at the limit: parsed.
    let job = buf.syntax.update(&buf.rope, text.len()).expect("parsed at the limit");
    buf.syntax.parsed(job.run());
    assert!(buf.syntax.tree().is_some());
    // One byte over: no tree.
    edit(&mut buf, vec![change(0, 0, " ")]);
    assert!(buf.syntax.update(&buf.rope, text.len()).is_none());
    assert!(buf.syntax.too_large() && buf.syntax.tree().is_none());
    // 0 turns parsing off, however small the text.
    let mut buf = Buffer::from_str("");
    buf.syntax.set_language(Language::C);
    assert!(buf.syntax.update(&buf.rope, 0).is_none());
    assert!(buf.syntax.too_large());
}

#[test]
fn too_many_queued_edits_restart_the_parse() {
    let mut buf = Buffer::from_str(&"int a;\n".repeat(COALESCE_AT));
    buf.syntax.set_language(Language::C);
    let job = buf.syntax.update(&buf.rope, MAX).unwrap();
    // Sets of `COALESCE_AT` changes (an `InputEdit` each) while the parse runs.
    let set = || (0..COALESCE_AT).map(|i| change(i * 7 + 4, i * 7 + 5, "b")).collect();
    for _ in 0..MAX_QUEUED / COALESCE_AT {
        edit(&mut buf, set());
    }
    assert!(!job.cancel.load(Ordering::Relaxed), "{MAX_QUEUED} queued");
    edit(&mut buf, set());
    assert!(job.cancel.load(Ordering::Relaxed), "one more set restarts the parse");
    buf.syntax.parsed(job.run());
    settle(&mut buf);
    assert_matches_fresh(&buf, "restarted");
}

/// `scope_before` across the grammars' node kinds: `|` marks each position asked about.
#[test]
fn scopes_in_every_language() {
    use Language::*;
    use Scope::*;
    let cases: &[(Language, &str, &[Scope])] = &[
        (
            Rust,
            "let s = \"a|\"; // c|\n/* b| */ x|",
            &[String, Comment, Comment, Code],
        ),
        (Rust, "let c = 'a|';", &[String]),
        (Json, "{\"k|\": 1|}", &[String, Code]),
        (Jsonc, "{} // c|\n", &[Comment]),
        (
            Yaml,
            "a: \"b|\"\nc: 'd|'\ne: f|  # g|\n",
            &[String, String, String, Comment],
        ),
        (Toml, "a = \"b|\" # c|\n", &[String, Comment]),
        (
            ShellScript,
            "echo \"a|$(b|)\" 'c|' # d|\n",
            &[String, Code, String, Comment],
        ),
        (ShellScript, "echo \"${x|}\"\n", &[Code]),
        (ShellScript, "cat <<EOF\na|\nEOF\n", &[String]),
        (Yaml, "a: >\n  b|\n", &[String]),
        (CMake, "set(a \"b|\" c|) # d|\n", &[String, Code, Comment]),
        (CMake, "set(a [[b|]] \"${c|}\")\n", &[String, Code]),
        (Makefile, "a = b|\n# c|\n", &[Code, Comment]),
        (Dockerfile, "# c|\nENV A=\"b|\" C=d|\n", &[Comment, String, Code]),
        (GitCommit, "subj|\n\n# c|\n", &[Code, Comment]),
        (Xml, "<a x=\"y|\">t|</a><!-- c| -->", &[String, Code, Comment]),
        (Markdown, "# a|\n", &[Code]),
        (Cpp, "f(\"a|\"); // b|\n", &[String, Comment]),
        (ObjC, "x = @\"a|\"; // b|\n/* c| */\n", &[String, Comment, Comment]),
        (ObjCpp, "x = @\"a|\"; // b|\n", &[String, Comment]),
        (
            Swift,
            "let s = \"a|\\(b|)c|\" // d|\nlet m = \"\"\"\nx|\n\"\"\"\n/* e| */\n",
            &[String, Code, String, Comment, String, Comment],
        ),
        // Raw strings and their `\#(…)` interpolation, a string inside an interpolation, a
        // nested block comment (still a comment after the inner `*/`), `#keyPath(…)` is code, and
        // so is right after a `\(` (the opener is a leaf of the string).
        (
            Swift,
            "let r = #\"a|\\#(b|)c|\"#\nlet q = \"\\(f(\"x|\"))\"\n/* a /* b */ c| */ d|\nlet k = #keyPath(A.b|)\nlet z = \"\\(| b)\"\n",
            &[String, Code, String, String, Comment, Code, Code, Code],
        ),
        (Python, "f'a{b|}c|'  # d|\n", &[Code, String, Comment]),
    ];
    for &(language, marked, want) in cases {
        let text = marked.replace('|', "");
        let buf = parsed_buffer(language, &text);
        let positions = marked.match_indices('|').enumerate().map(|(i, (at, _))| at - i);
        let got: Vec<_> = positions
            .map(|p| buf.syntax.scope_before(ByteIdx(p)).unwrap())
            .collect();
        assert_eq!(got, want, "{language:?} {marked:?}");
    }
}

#[test]
fn scope_kinds() {
    assert_eq!(scope_of("line_comment"), Some(Scope::Comment));
    assert_eq!(scope_of("Comment"), Some(Scope::Comment));
    assert_eq!(scope_of("raw_string_literal"), Some(Scope::String));
    assert_eq!(scope_of("double_quote_scalar"), Some(Scope::String));
    assert_eq!(scope_of("unquoted_string"), None);
    assert_eq!(scope_of("command_substitution"), Some(Scope::Code));
    assert_eq!(scope_of("interpolated_expression"), Some(Scope::Code));
    assert_eq!(scope_of("raw_str_interpolation"), Some(Scope::Code));
    assert_eq!(scope_of("key_path_string_expression"), Some(Scope::Code));
    assert_eq!(scope_of("identifier"), None);
}

/// Markdown's block tree is current but not a token tree: brackets come from the text.
#[test]
fn markdown_has_no_token_tree() {
    let buf = parsed_buffer(Language::Markdown, "a (b)\n");
    assert!(buf.syntax.tree().is_some());
    assert!(buf.syntax.token_tree().is_none() && buf.syntax.tokens().is_none());
    let buf = parsed_buffer(Language::Rust, "fn f() {}\n");
    assert!(buf.syntax.token_tree().is_some());
}
