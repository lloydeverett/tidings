//! Which strings are allowed as a Path or a Prefix. The rules don't depend on the Backend, so they
//! are checked here once, as a table; the behaviour suite checks that every operation applies them.

use tidings::{Error, InvalidPathReason, Path, Prefix};

/// Strings every platform accepts as a Path.
const ACCEPTED: &[&str] = &[
    "settings.toml",
    "themes/dark.toml",
    "a/b/c/d.txt",
    ".hidden",
    "notes/.draft.md",
    "with space.txt",
    " leading space",
    "semi;colon.js",
    "single'quote",
    "console.log",
    "CONFIG",
    "com10",
    "CONIN.txt",
    "not CON.txt",
    "a.b.c",
    // Unicode, in NFC form.
    "caf\u{e9}/r\u{e9}sum\u{e9}.md",
    "\u{65e5}\u{672c}\u{8a9e}/\u{30e1}\u{30e2}.txt",
    "\u{1f600}.txt",
    // Only the top-level `.tidings` is reserved.
    "notes/.tidings",
    ".tidings2",
    ".tidings.toml",
    // Names only like those of tidings' temporary files.
    ".settings.toml.tidings-backup",
    "settings.toml.tidings-0123456789abcdef0123456789abcdef-0",
    ".settings.toml.tidings-0123456789abcdef-0",
    ".settings.toml.tidings-0123456789abcdef0123456789abcdef",
    ".settings.toml.tidings-0123456789abcdef0123456789abcdef-x",
    "..tidings-0123456789abcdef0123456789abcdef-0",
];

/// Strings that are refused as a Path, with the rule each one breaks.
const REFUSED: &[(&str, InvalidPathReason)] = &[
    ("", InvalidPathReason::Empty),
    ("/etc/passwd", InvalidPathReason::NotRelative),
    ("a//b", InvalidPathReason::EmptySegment),
    ("trailing/", InvalidPathReason::EmptySegment),
    (".", InvalidPathReason::DotSegment),
    ("..", InvalidPathReason::DotSegment),
    ("./a.txt", InvalidPathReason::DotSegment),
    ("a/../b.txt", InvalidPathReason::DotSegment),
    ("a/.", InvalidPathReason::DotSegment),
    // Names Windows reserves, in any letter case and with any extension.
    ("CON", InvalidPathReason::UnportableName),
    ("aux.txt", InvalidPathReason::UnportableName),
    ("logs/nul", InvalidPathReason::UnportableName),
    ("Com1.tar.gz", InvalidPathReason::UnportableName),
    ("lpt9", InvalidPathReason::UnportableName),
    ("prn/a.txt", InvalidPathReason::UnportableName),
    // More reserved names, which sanitize-filename misses: the superscript-digit ports, the
    // console's own names, and any reserved name with spaces before its extension.
    ("COM\u{b9}", InvalidPathReason::UnportableName),
    ("com\u{b2}", InvalidPathReason::UnportableName),
    ("COM\u{b3}.txt", InvalidPathReason::UnportableName),
    ("LPT\u{b9}", InvalidPathReason::UnportableName),
    ("lpt\u{b2}.log", InvalidPathReason::UnportableName),
    ("LPT\u{b3}", InvalidPathReason::UnportableName),
    ("CONIN$", InvalidPathReason::UnportableName),
    ("conout$.txt", InvalidPathReason::UnportableName),
    ("CON .txt", InvalidPathReason::UnportableName),
    ("NUL .txt", InvalidPathReason::UnportableName),
    ("aux  .tar.gz", InvalidPathReason::UnportableName),
    // Characters Windows doesn't allow in a name.
    ("back\\slash", InvalidPathReason::UnportableName),
    ("C:/Windows", InvalidPathReason::UnportableName),
    ("what?", InvalidPathReason::UnportableName),
    ("star*", InvalidPathReason::UnportableName),
    ("a<b>c", InvalidPathReason::UnportableName),
    ("pipe|", InvalidPathReason::UnportableName),
    ("\"quoted\"", InvalidPathReason::UnportableName),
    // Control characters.
    ("line\nbreak", InvalidPathReason::UnportableName),
    ("tab\there", InvalidPathReason::UnportableName),
    ("nul\u{0}byte", InvalidPathReason::UnportableName),
    ("c1\u{85}control", InvalidPathReason::UnportableName),
    ("del\u{7f}", InvalidPathReason::UnportableName),
    ("\u{7f}", InvalidPathReason::UnportableName),
    // A trailing dot or space, which Windows drops.
    ("trailing.", InvalidPathReason::UnportableName),
    ("trailing ", InvalidPathReason::UnportableName),
    ("dir./a.txt", InvalidPathReason::UnportableName),
    ("...", InvalidPathReason::UnportableName),
    // Not in NFC form: `e` followed by a combining acute accent, where NFC has the single `\u{e9}`.
    ("cafe\u{301}.txt", InvalidPathReason::NotNfc),
    ("notes/re\u{301}sume\u{301}", InvalidPathReason::NotNfc),
    // A Hangul syllable as its separate jamo.
    ("\u{1100}\u{1161}.txt", InvalidPathReason::NotNfc),
    // tidings' own bookkeeping, in any letter case since some filesystems ignore it.
    (".tidings", InvalidPathReason::Reserved),
    (".tidings/lock", InvalidPathReason::Reserved),
    (".TIDINGS/journal", InvalidPathReason::Reserved),
    // Long s and dotless i, which uppercase to `S` and `I`.
    (".tiding\u{17f}/lock", InvalidPathReason::Reserved),
    (".t\u{131}dings", InvalidPathReason::Reserved),
    // The names of tidings' temporary files, anywhere, so that no File can be taken for one.
    (".settings.toml.tidings-0123456789abcdef0123456789abcdef-0", InvalidPathReason::Reserved),
    ("themes/.dark.toml.tidings-0123456789abcdef0123456789abcdef-12", InvalidPathReason::Reserved),
    (".a.tidings-0123456789abcdef0123456789abcdef-3/b.txt", InvalidPathReason::Reserved),
];

#[test]
fn accepted_paths_are_kept_as_given() {
    for &path in ACCEPTED {
        match Path::new(path) {
            Ok(valid) => assert_eq!(valid.as_str(), path),
            Err(error) => panic!("{path:?} should be accepted, got {error}"),
        }
    }
}

#[test]
fn refused_paths_name_the_rule_they_break() {
    for &(path, expected) in REFUSED {
        match Path::new(path) {
            Err(Error::InvalidPath { path: given, reason }) => {
                assert_eq!(reason, expected, "{path:?}");
                assert_eq!(given, path);
            }
            other => panic!("{path:?} should be refused, got {other:?}"),
        }
    }
}

/// Prefixes that are allowed or refused because of what a Prefix is, beyond the rules for Paths.
const PREFIXES: &[(&str, Option<InvalidPathReason>)] = &[
    // The empty Prefix: the whole Area.
    ("", None),
    ("themes/", None),
    ("notes/2026/", None),
    // A Prefix ends at a `/`, so that `themes` can't be mistaken for a File.
    ("themes", Some(InvalidPathReason::NoTrailingSlash)),
    ("notes/2026", Some(InvalidPathReason::NoTrailingSlash)),
    ("/", Some(InvalidPathReason::NotRelative)),
    ("//", Some(InvalidPathReason::NotRelative)),
];

#[test]
fn a_prefix_is_empty_or_ends_with_a_slash() {
    for &(prefix, expected) in PREFIXES {
        assert_prefix(prefix, expected);
    }
}

#[test]
fn prefixes_follow_the_rules_for_paths() {
    for &path in ACCEPTED {
        assert_prefix(&format!("{path}/"), None);
    }
    for &(path, expected) in REFUSED.iter().filter(|(path, _)| !path.is_empty()) {
        assert_prefix(&format!("{path}/"), Some(expected));
    }
}

fn assert_prefix(prefix: &str, expected: Option<InvalidPathReason>) {
    match (Prefix::new(prefix), expected) {
        (Ok(valid), None) => assert_eq!(valid.as_str(), prefix),
        (Err(Error::InvalidPath { path: given, reason }), Some(expected)) => {
            assert_eq!(reason, expected, "{prefix:?}");
            assert_eq!(given, prefix);
        }
        (result, _) => panic!("Prefix {prefix:?}: expected {expected:?}, got {result:?}"),
    }
}

#[test]
fn a_name_longer_than_255_bytes_is_refused() {
    let longest = "é".repeat(127) + "a";
    assert_eq!(longest.len(), 255);
    assert!(Path::new(format!("notes/{longest}")).is_ok());

    let too_long = "é".repeat(128);
    match Path::new(format!("notes/{too_long}")) {
        Err(Error::InvalidPath { reason, .. }) => {
            assert_eq!(reason, InvalidPathReason::UnportableName)
        }
        other => panic!("a 256-byte name should be refused, got {other:?}"),
    }
}
