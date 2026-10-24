//! Syntax highlighting to ANSI escape strings, one string per source line.

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};

fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    if r == g && g == b {
        if r < 8 {
            return 16;
        }
        if r > 248 {
            return 231;
        }
        return 232 + ((r as u16 - 8) * 24 / 247) as u8;
    }
    let q = |v: u8| ((v as u16 * 5 + 127) / 255) as u8;
    16 + 36 * q(r) + 6 * q(g) + q(b)
}

/// SGR sequence for a syntect style. Handles the special alpha encoding used by the
/// `ansi` / `base16` themes (a == 0: palette index in `r`, a == 1: default colour).
pub fn sgr(style: &Style, truecolor: bool) -> String {
    let fg = style.foreground;
    let mut codes = String::from("0");
    match fg.a {
        0 => {
            if fg.r < 8 {
                codes.push_str(&format!(";{}", 30 + fg.r));
            } else if fg.r < 16 {
                codes.push_str(&format!(";{}", 90 + fg.r - 8));
            } else {
                codes.push_str(&format!(";38;5;{}", fg.r));
            }
        }
        1 => codes.push_str(";39"),
        _ => {
            if truecolor {
                codes.push_str(&format!(";38;2;{};{};{}", fg.r, fg.g, fg.b));
            } else {
                codes.push_str(&format!(";38;5;{}", rgb_to_256(fg.r, fg.g, fg.b)));
            }
        }
    }
    if style.font_style.contains(FontStyle::BOLD) {
        codes.push_str(";1");
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        codes.push_str(";3");
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        codes.push_str(";4");
    }
    format!("\x1b[{codes}m")
}

pub fn highlight_lines(
    text: &str,
    ss: &SyntaxSet,
    syntax: &SyntaxReference,
    theme: &Theme,
    truecolor: bool,
    limit: usize,
) -> Vec<String> {
    let mut h = HighlightLines::new(syntax, theme);
    let mut out = Vec::new();
    for raw in text.lines().take(limit) {
        let mut line = String::with_capacity(raw.len() + 1);
        line.push_str(raw);
        line.push('\n');
        match h.highlight_line(&line, ss) {
            Ok(ranges) => {
                let mut s = String::new();
                for (style, piece) in ranges {
                    let piece = piece.trim_end_matches(['\n', '\r']);
                    if piece.is_empty() {
                        continue;
                    }
                    s.push_str(&sgr(&style, truecolor));
                    s.push_str(piece);
                }
                s.push_str("\x1b[0m");
                out.push(s);
            }
            Err(_) => out.push(raw.to_string()),
        }
    }
    out
}

/// Remove up to `n` leading whitespace characters from a highlighted line,
/// skipping over escape sequences.
pub fn strip_visible_prefix(s: &str, n: usize) -> String {
    let mut out = String::with_capacity(s.len());
    let mut removed = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            out.push(c);
            for d in chars.by_ref() {
                out.push(d);
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else if removed < n && (c == ' ' || c == '\t') {
            removed += 1;
        } else {
            out.push(c);
            out.extend(chars);
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_through_escapes() {
        let s = "\x1b[0;38;5;1m    \x1b[0mfoo";
        assert_eq!(strip_visible_prefix(s, 2), "\x1b[0;38;5;1m  \x1b[0mfoo");
        assert_eq!(strip_visible_prefix(s, 4), "\x1b[0;38;5;1m\x1b[0mfoo");
    }
}
