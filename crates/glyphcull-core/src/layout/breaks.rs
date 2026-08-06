//! Text breaking — word boundaries and line-break opportunities (mirrors the
//! JS `src/layout/breaks.ts`).
//!
//! Scope (documented in DESIGN.md D8 and TESTING.md): the runtime breaks
//! Latin/Cyrillic/Greek and mark-heavy text at UAX #29-grounded word
//! boundaries, and CJK text between ideographs (no dictionary needed for
//! break *opportunities*). The rules implemented here are the v1 subset:
//!
//! - A break is allowed after whitespace runs.
//! - A break is forbidden inside a word (letters, digits, underscores,
//!   apostrophes in contractions).
//! - A break is forbidden before closing punctuation
//!   `, . ; : ! ? ) ] } %` and after opening punctuation `( [ {` and quotes.
//! - Combining marks bind to their base (no break between base and mark).
//! - CJK ideographs/hiragana/katakana/hangul: break allowed between any two;
//!   forbidden before closing CJK punctuation and after opening CJK
//!   punctuation.
//! - Explicit newlines are forced breaks (preformatted content).
//!
//! The token stream feeds the Knuth–Plass line breaker (`kp.rs`).

use unicode_general_category::{get_general_category, GeneralCategory};
use unicode_script::{Script, UnicodeScript};

/// How a break after a token is classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BreakClass {
    /// A space: the natural break opportunity (glue).
    Space,
    /// A permissible break with no glue (CJK between-ideograph).
    Allowed,
    /// No break here (inside a word, before punctuation, …).
    Forbidden,
    /// A forced break (explicit newline).
    Forced,
}

/// One text token for the line breaker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextToken {
    /// The token's text.
    pub text: String,
    /// How a break after the token is classified.
    pub break_after: BreakClass,
}

/// Unicode White_Space is close to but not identical with ECMAScript `\s`;
/// the JS runtime uses `\s`, so the mirror must be exact: `Zs` plus TAB, VT,
/// FF, LF, CR, LS, PS, and ZWNBSP (U+FEFF). Rust's `char::is_whitespace`
/// additionally matches NEL (U+0085) and omits ZWNBSP, either of which would
/// change a whitespace-run's extent.
fn is_space(ch: char) -> bool {
    matches!(get_general_category(ch), GeneralCategory::SpaceSeparator)
        || matches!(
            ch,
            '\u{0009}'
                | '\u{000b}'
                | '\u{000c}'
                | '\u{000a}'
                | '\u{000d}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{feff}'
        )
}

/// `\p{L}\p{N}_\u{2019}`: letters, digits, underscore, right single quote.
/// (`char::is_alphabetic`/`is_numeric` are supersets of `\p{L}`/`\p{N}` —
/// e.g. Other_Alphabetic marks and the Letter_Number category — so the
/// general-category match below is the exact mirror.)
fn is_word_char(ch: char) -> bool {
    matches!(
        get_general_category(ch),
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
            | GeneralCategory::DecimalNumber
            | GeneralCategory::LetterNumber
            | GeneralCategory::OtherNumber
    ) || ch == '_'
        || ch == '\u{2019}'
}

/// `\p{P}\p{S}`: punctuation or symbols (the JS `PUNCTUATION` class).
fn is_punctuation_or_symbol(ch: char) -> bool {
    let category = get_general_category(ch);
    matches!(
        category,
        GeneralCategory::ClosePunctuation
            | GeneralCategory::ConnectorPunctuation
            | GeneralCategory::DashPunctuation
            | GeneralCategory::FinalPunctuation
            | GeneralCategory::InitialPunctuation
            | GeneralCategory::OpenPunctuation
            | GeneralCategory::OtherPunctuation
            | GeneralCategory::CurrencySymbol
            | GeneralCategory::ModifierSymbol
            | GeneralCategory::MathSymbol
            | GeneralCategory::OtherSymbol
    )
}

/// Closing punctuation that binds to the preceding character (the JS
/// `NO_BREAK_BEFORE` class).
fn is_no_break_before(ch: char) -> bool {
    matches!(
        ch,
        ',' | '.'
            | '!'
            | '?'
            | ';'
            | ':'
            | '%'
            | '\u{2026}' // …
            | ')'
            | ']'
            | '}'
            | '\u{3001}' // 、
            | '\u{3002}' // 。
            | '\u{ff09}' // ）
            | '\u{ff3d}' // ］
            | '\u{300b}' // 》
            | '\u{300d}' // 』
            | '\u{3011}' // 】
            | '\u{ff1f}' // ？
            | '\u{ff01}' // ！
            | '\u{ff1b}' // ；
            | '\u{ff1a}' // ：
    )
}

/// Opening punctuation and quotes that bind to the following character (the
/// JS `NO_BREAK_AFTER` class).
fn is_no_break_after(ch: char) -> bool {
    matches!(
        ch,
        '(' | '['
            | '"'
            | '\u{2018}' // '
            | '\u{201c}' // "
            | '\u{00ab}' // «
            | '\u{300c}' // 「
            | '\u{300e}' // 『
            | '\u{3008}' // 〈
            | '\u{300a}' // 《
            | '\u{ff08}' // （
            | '\u{ff3b}' // ［
            | '\u{3010}' // 【
            | '\u{2019}' // '
    )
}

/// CJK scripts: Han, Hiragana, Katakana, Hangul (the JS `CJK` class).
fn is_cjk(ch: char) -> bool {
    matches!(
        ch.script(),
        Script::Han | Script::Hiragana | Script::Katakana | Script::Hangul
    )
}

/// Tokenize text into breakable units. Whitespace runs become separate
/// tokens with [`BreakClass::Space`]; CJK characters become individual tokens
/// with [`BreakClass::Allowed`] breaks between them; word runs stay intact.
///
/// All `chars[i]`/`chars[i..j]` accesses are provably in bounds: `i` and `j`
/// are maintained within `0..=n` by the scan loops (scoped allow).
#[must_use]
#[allow(clippy::indexing_slicing)]
pub fn tokenize_for_breaking(text: &str) -> Vec<TextToken> {
    let mut tokens: Vec<TextToken> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let cp = chars[i];

        if cp == '\n' || cp == '\r' {
            tokens.push(TextToken {
                text: cp.to_string(),
                break_after: BreakClass::Forced,
            });
            i += 1;
            continue;
        }
        if is_space(cp) {
            // A whitespace run: one glue token.
            let mut j = i;
            while j < n && is_space(chars[j]) {
                j += 1;
            }
            tokens.push(TextToken {
                text: chars[i..j].iter().collect(),
                break_after: BreakClass::Space,
            });
            i = j;
            continue;
        }
        if is_cjk(cp) {
            // CJK: each char is its own box; breaks are allowed between them.
            // Closing CJK punctuation binds to the preceding character (no
            // break between them, UAX #29 CL/NS semantics). Note: the JS
            // checks `PUNCTUATION && NO_BREAK_BEFORE` here; NO_BREAK_BEFORE is
            // itself a subset of P/S, so the class check is equivalent.
            let mut j = i + 1;
            while j < n && is_no_break_before(chars[j]) {
                j += 1;
            }
            tokens.push(TextToken {
                text: chars[i..j].iter().collect(),
                break_after: if j < n {
                    BreakClass::Allowed
                } else {
                    BreakClass::Forbidden
                },
            });
            i = j;
            continue;
        }
        // A word run: letters/digits/underscore/contraction apostrophe.
        let mut j = i;
        while j < n {
            let next_ch = chars[j];
            if is_space(next_ch) || is_cjk(next_ch) || next_ch == '\n' || next_ch == '\r' {
                break;
            }
            if is_word_char(next_ch) {
                j += 1;
                continue;
            }
            if is_punctuation_or_symbol(next_ch) {
                if is_no_break_before(next_ch) {
                    // Closing punctuation binds to the preceding word (and its
                    // break-after is still decided by what follows).
                    j += 1;
                    continue;
                }
                if is_no_break_after(next_ch) && j == i {
                    // An opening punctuation at the token start binds to the
                    // following word (a break is forbidden after it):
                    // '('hello' → one token.
                    j += 1;
                    continue;
                }
                break;
            }
            break;
        }
        // Defensive: never emit an empty token — the scan always consumes at
        // least the current character (a symbol that binds neither way).
        if j == i {
            j = i + 1;
        }
        // Decide the break after this word based on the next character.
        let break_after = if j < n {
            if is_no_break_after(chars[j]) {
                // An opener follows: the word binds to it; the break happens
                // later.
                BreakClass::Forbidden
            } else {
                BreakClass::Allowed
            }
        } else {
            BreakClass::Forbidden // end of text
        };
        tokens.push(TextToken {
            text: chars[i..j].iter().collect(),
            break_after,
        });
        i = j;
    }
    tokens
}

/// Whether a codepoint is a combining mark (Mn / Me / Mc). The common
/// combining ranges cover v1 glyph attachment (marks advance 0 in the atlas).
#[must_use]
pub fn is_combining_mark(cp: u32) -> bool {
    (0x0300..=0x036f).contains(&cp) // Combining Diacritical Marks
        || (0x1ab0..=0x1aff).contains(&cp) // Combining Diacritical Marks Extended
        || (0x1dc0..=0x1dff).contains(&cp) // Combining Diacritical Marks Supplement
        || (0x20d0..=0x20ff).contains(&cp) // Combining Diacritical Marks for Symbols
        || (0xfe20..=0xfe2f).contains(&cp) // Combining Half Marks
}
