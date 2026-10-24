//! Language detection: by extension, by `-t` token, or by guessing from content.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Python,
    Rust,
    Go,
    JavaScript,
}

impl Lang {
    pub fn name(self) -> &'static str {
        match self {
            Lang::Python => "Python",
            Lang::Rust => "Rust",
            Lang::Go => "Go",
            Lang::JavaScript => "JavaScript",
        }
    }

    /// Token understood by syntect's `find_syntax_by_token`.
    pub fn syntax_token(self) -> &'static str {
        match self {
            Lang::Python => "py",
            Lang::Rust => "rs",
            Lang::Go => "go",
            Lang::JavaScript => "js",
        }
    }

    pub fn from_path(path: &Path) -> Option<Lang> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Lang::from_token(&ext)
    }

    pub fn from_token(token: &str) -> Option<Lang> {
        match token.trim().to_ascii_lowercase().as_str() {
            "py" | "pyw" | "pyi" | "python" | "python3" => Some(Lang::Python),
            "rs" | "rust" => Some(Lang::Rust),
            "go" | "golang" => Some(Lang::Go),
            "js" | "mjs" | "cjs" | "jsx" | "javascript" | "node" => Some(Lang::JavaScript),
            _ => None,
        }
    }

    /// Heuristic for clipboard / stdin content that has no file name.
    pub fn guess(text: &str) -> Option<Lang> {
        fn score(text: &str, needles: &[&str]) -> usize {
            needles.iter().filter(|n| text.contains(**n)).count()
        }
        let candidates = [
            (
                Lang::Rust,
                score(
                    text,
                    &[
                        "fn ",
                        "impl ",
                        "let mut ",
                        "pub fn",
                        "use std::",
                        "&self",
                        "#[derive",
                        "-> ",
                    ],
                ),
            ),
            (
                Lang::Go,
                score(
                    text,
                    &["package ", "func ", ":= ", "import (", "fmt.", "go func"],
                ),
            ),
            (
                Lang::Python,
                score(
                    text,
                    &[
                        "def ", "import ", "self", "elif ", "__init__", "print(", "class ", "):\n",
                        "None",
                    ],
                ),
            ),
            (
                Lang::JavaScript,
                score(
                    text,
                    &[
                        "function ",
                        "const ",
                        "=> ",
                        "console.log",
                        "require(",
                        "export ",
                        "let ",
                        "var ",
                    ],
                ),
            ),
        ];
        let (lang, best) = candidates
            .iter()
            .copied()
            .max_by_key(|(_, s)| *s)
            .filter(|(_, s)| *s >= 2)?;
        let _ = best;
        Some(lang)
    }
}
