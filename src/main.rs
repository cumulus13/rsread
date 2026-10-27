//! File: src\main.rs
//! Author: Hadi Cahyadi <cumulus13@gmail.com>
//! Date: 2026-10-27
//! Description: rsread — fast multi-language code reader (Rust port and extension of pyread).
//! License: MIT


mod analyzer;
mod clip;
mod git;
mod highlight;
mod lang;
mod render;
mod themes;

use anyhow::{anyhow, bail, Context, Result};
use clap::{CommandFactory, Parser, ValueEnum};
use clap_version_flag::colorful_version;
use clap_color_help::default_styles;
use colored::Colorize;
use lang::Lang;
use render::{emit, Renderer, View};
use serde::Serialize;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use syntect::highlighting::Theme;
use syntect::parsing::{SyntaxReference, SyntaxSet};
use themes::Themes;

const EXAMPLES: &str = "\
EXAMPLES:
  rsread src/main.rs                    structure tree + duplicate warnings + Git markers
  rsread app.py -m Class.method         extract a method (also: Type::method, re:^get_)
  rsread app.go -L 25 50                lines 25..50 (also -L 25-50)
  rsread app.js -c -s nord              whole file, 'nord' theme
  rsread src/ -m parse                  search a name across a whole directory
  rsread app.py -d                      duplicate analysis only (exit code 2 if redefinitions)
  rsread c                              analyze the clipboard (then offers to save it)
  cat x.py | rsread - --json            machine readable structure

Supported structure analysis: Python, Rust, Go, JavaScript. Other files get highlighting only.
Debug: set TRACEBACK=1 for full error chains.";

#[derive(Copy, Clone, ValueEnum, PartialEq, Eq)]
enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Parser)]
#[command(
    name = "rsread",
    disable_version_flag = true,
    about = "Fast multi-language code reader: structure tree, syntax highlighting, duplicate detection and Git change markers",
    after_help = EXAMPLES,
    styles = defaut_styles()
)]
struct Cli {
    /// File(s) or directories to analyze; `c` = clipboard, `-` = stdin
    #[arg(value_name = "FILE")]
    files: Vec<String>,

    /// Show a method/function/class by name (repeatable). Accepts `Class.method`, `Type::method`, `re:<regex>`
    #[arg(short = 'm', long = "method", value_name = "NAME")]
    methods: Vec<String>,

    /// Syntax highlighting theme (see -l)
    #[arg(
        short = 's',
        long = "style",
        value_name = "THEME",
        default_value = "Dracula",
        env = "RSREAD_STYLE"
    )]
    style: String,

    /// List all available themes with a live preview
    #[arg(short = 'l', long = "list-themes")]
    list_themes: bool,

    /// Force the language / syntax (py, rs, go, js, ...)
    #[arg(short = 't', long = "type", value_name = "TYPE")]
    syntax_type: Option<String>,

    /// Display the entire source code
    #[arg(short = 'c', long = "code")]
    code: bool,

    /// Show duplicate analysis only
    #[arg(short = 'd', long = "duplicates")]
    duplicates: bool,

    /// Show a line or range: `-L 10`, `-L 10 20`, `-L 10-20`
    #[arg(short = 'L', long = "lines", num_args = 1..=2, value_names = ["START", "END"])]
    lines: Vec<String>,

    /// Hide line numbers (alias: -nl)
    #[arg(short = 'n', long = "no-linenumber")]
    no_linenumber: bool,

    /// Compact output: no left padding, extracted code is dedented
    #[arg(short = 'S', long = "strip")]
    strip: bool,

    /// Disable Git change detection
    #[arg(long = "no-git")]
    no_git: bool,

    /// Compare against this revision instead of HEAD
    #[arg(long = "git-rev", value_name = "REV", default_value = "HEAD")]
    git_rev: String,

    /// Copy the extracted code (-m, -L, -c) to the clipboard
    #[arg(short = 'y', long = "copy")]
    copy: bool,

    /// Print the structure as JSON
    #[arg(long = "json")]
    json: bool,

    /// Use plain ASCII markers instead of emoji
    #[arg(long = "no-emoji", env = "RSREAD_NO_EMOJI")]
    no_emoji: bool,

    /// When to use colors
    #[arg(long = "color", value_enum, default_value_t = ColorMode::Auto)]
    color: ColorMode,
}

// ---------------------------------------------------------------------------
// inputs
// ---------------------------------------------------------------------------

struct Input {
    label: String,
    path: Option<PathBuf>,
    text: String,
    clipboard: bool,
    lang: Option<Lang>,
}

fn decode(bytes: Vec<u8>) -> (String, Option<&'static str>) {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(&bytes[3..]).into_owned(), None);
    }
    let utf16 = |b: &[u8], le: bool| {
        let units = b.chunks_exact(2).map(|c| {
            if le {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        });
        char::decode_utf16(units)
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect::<String>()
    };
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return (utf16(&bytes[2..], true), None);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return (utf16(&bytes[2..], false), None);
    }
    match String::from_utf8(bytes) {
        Ok(s) => (s, None),
        Err(e) => (
            String::from_utf8_lossy(e.as_bytes()).into_owned(),
            Some("not valid UTF-8, decoded with replacement characters"),
        ),
    }
}

fn display_label(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix("./")
        .or_else(|| s.strip_prefix(".\\"))
        .unwrap_or(&s)
        .to_string()
}

fn make_input(
    cli: &Cli,
    label: String,
    path: Option<PathBuf>,
    text: String,
    clipboard: bool,
) -> Input {
    let lang = match &cli.syntax_type {
        Some(t) => Lang::from_token(t),
        None => path.as_deref().and_then(Lang::from_path).or_else(|| {
            if path.is_none() {
                Lang::guess(&text)
            } else {
                None
            }
        }),
    };
    Input {
        label,
        path,
        text,
        clipboard,
        lang,
    }
}

fn read_path(cli: &Cli, path: &Path) -> Result<Input> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut msg = format!("file not found: {}", path.display());
            let dir = match path.parent() {
                Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                _ => PathBuf::from("."),
            };
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !stem.is_empty() {
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    let near: Vec<String> = rd
                        .flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|n| n.to_lowercase().contains(&stem))
                        .take(3)
                        .collect();
                    if !near.is_empty() {
                        msg.push_str(&format!("\n   did you mean: {}?", near.join(", ")));
                    }
                }
            }
            bail!(msg);
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            bail!(
                "permission denied: {} (check the file permissions)",
                path.display()
            )
        }
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    if bytes.iter().take(8192).any(|b| *b == 0)
        && !bytes.starts_with(&[0xFF, 0xFE])
        && !bytes.starts_with(&[0xFE, 0xFF])
    {
        bail!("{} looks like a binary file", path.display());
    }
    let (text, note) = decode(bytes);
    if let Some(n) = note {
        eprintln!("{} {}: {}", "warning:".yellow().bold(), path.display(), n);
    }
    Ok(make_input(
        cli,
        display_label(path),
        Some(path.to_path_buf()),
        text,
        false,
    ))
}

fn gather(cli: &Cli) -> Result<Vec<Input>> {
    let mut out = Vec::new();
    for arg in &cli.files {
        if arg == "-" {
            let mut buf = Vec::new();
            std::io::stdin()
                .read_to_end(&mut buf)
                .context("cannot read stdin")?;
            let (text, _) = decode(buf);
            out.push(make_input(cli, "<stdin>".into(), None, text, false));
        } else if arg == "c" && !Path::new("c").exists() {
            let text = clip::get()?;
            if text.trim().is_empty() {
                bail!("clipboard is empty");
            }
            out.push(make_input(cli, "<clipboard>".into(), None, text, true));
        } else if Path::new(arg).is_dir() {
            let mut files: Vec<PathBuf> = ignore::WalkBuilder::new(arg)
                .build()
                .flatten()
                .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
                .map(|e| e.into_path())
                .filter(|p| Lang::from_path(p).is_some())
                .collect();
            files.sort();
            if files.is_empty() {
                eprintln!(
                    "{} no supported source files in {}",
                    "warning:".yellow().bold(),
                    arg
                );
            }
            for f in files {
                match read_path(cli, &f) {
                    Ok(i) => out.push(i),
                    Err(e) => eprintln!("{} {e}", "warning:".yellow().bold()),
                }
            }
        } else {
            out.push(read_path(cli, Path::new(arg))?);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// app
// ---------------------------------------------------------------------------

struct App<'a> {
    cli: &'a Cli,
    r: Renderer,
    ss: SyntaxSet,
    theme: Theme,
    color: bool,
    truecolor: bool,
    view: View,
    range: Option<(usize, Option<usize>)>,
}

#[derive(Default)]
struct Outcome {
    matched: bool,
    redefs: usize,
    copied: String,
    json: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct JsonItem<'a> {
    #[serde(flatten)]
    item: &'a analyzer::Item,
    git: Option<git::ItemGit>,
}

fn json_groups(a: &analyzer::Analysis, g: &[analyzer::DupGroup]) -> Vec<serde_json::Value> {
    g.iter()
        .map(|d| {
            serde_json::json!({
                "name": d.name,
                "lines": d.ids.iter().map(|i| a.items[*i].def_line).collect::<Vec<_>>(),
            })
        })
        .collect()
}

impl<'a> App<'a> {
    fn pick_syntax(&self, input: &Input) -> &SyntaxReference {
        let ss = &self.ss;
        if let Some(t) = &self.cli.syntax_type {
            if let Some(s) = ss.find_syntax_by_token(t) {
                return s;
            }
        }
        if let Some(ext) = input
            .path
            .as_deref()
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
        {
            if let Some(s) = ss.find_syntax_by_extension(ext) {
                return s;
            }
        }
        if let Some(l) = input.lang {
            if let Some(s) = ss.find_syntax_by_token(l.syntax_token()) {
                return s;
            }
        }
        if let Some(first) = input.text.lines().next() {
            if let Some(s) = ss.find_syntax_by_first_line(first) {
                return s;
            }
        }
        ss.find_syntax_plain_text()
    }

    /// Highlighted lines `1..=upto` (state must flow from the top, but nothing after `upto` is needed).
    fn highlighted(&self, input: &Input, upto: usize) -> Vec<String> {
        if !self.color {
            return input.text.lines().take(upto).map(String::from).collect();
        }
        highlight::highlight_lines(
            &input.text,
            &self.ss,
            self.pick_syntax(input),
            &self.theme,
            self.truecolor,
            upto,
        )
    }

    fn process(&self, input: &Input, multi: bool) -> Result<Outcome> {
        let cli = self.cli;
        let analysis = match input.lang {
            Some(l) => Some(analyzer::analyze(&input.text, l)?),
            None => None,
        };
        let git = if cli.no_git {
            None
        } else {
            input
                .path
                .as_deref()
                .and_then(|p| git::collect(p, &cli.git_rev))
        };
        let git = git.as_ref();
        let plain: Vec<&str> = input.text.lines().collect();
        let total = plain.len();
        let mut out = Outcome {
            matched: true,
            ..Default::default()
        };

        if multi && !cli.json {
            emit("");
            emit(format!(
                "{} {}",
                self.r.icon_dir(),
                input.label.bold().cyan()
            ));
        }

        // --json -------------------------------------------------------------
        if cli.json {
            let a = analysis
                .as_ref()
                .ok_or_else(|| anyhow!("--json needs a supported language (py, rs, go, js)"))?;
            let d = a.find_duplicates();
            let items: Vec<JsonItem> = a
                .items
                .iter()
                .map(|i| JsonItem {
                    item: i,
                    git: git.map(|g| g.range_stats(i.start, i.end)),
                })
                .collect();
            out.json = Some(serde_json::json!({
                "file": input.label,
                "language": a.lang.name().to_lowercase(),
                "lines": a.total_lines,
                "blank_lines": a.blank_lines,
                "syntax_error_lines": a.error_lines,
                "git": git.map(|g| serde_json::json!({"added": g.added, "modified": g.modified, "deleted": g.deleted})),
                "items": items,
                "duplicates": {
                    "redefinitions": json_groups(a, &d.redefinitions),
                    "identical_bodies": json_groups(a, &d.identical),
                    "shared_names": json_groups(a, &d.shared_names),
                },
            }));
            return Ok(out);
        }

        // --lines ------------------------------------------------------------
        if let Some((start, end)) = self.range {
            if total == 0 {
                bail!("{} is empty", input.label);
            }
            if start == 0 {
                bail!("line numbers start at 1");
            }
            if start > total {
                bail!(
                    "line {start} is beyond the end of {} ({total} lines)",
                    input.label
                );
            }
            let end = end.unwrap_or(start).min(total);
            if end < start {
                bail!("invalid range {start}-{end}");
            }
            let range_txt = if start == end {
                start.to_string()
            } else {
                format!("{start}-{end}")
            };
            emit(format!(
                "{}{} {} from: {}",
                self.r.icon_file().to_string() + " ",
                "Lines".bold(),
                range_txt.bold().cyan(),
                input.label.bold()
            ));
            if let Some(g) = git {
                let s = g.range_stats(start, end);
                if !s.is_empty() {
                    let mut parts = Vec::new();
                    if s.added > 0 {
                        parts.push(format!("+{} added", s.added).green().to_string());
                    }
                    if s.modified > 0 {
                        parts.push(format!("~{} modified", s.modified).yellow().to_string());
                    }
                    if s.deleted > 0 {
                        parts.push(format!("-{} deleted", s.deleted).red().to_string());
                    }
                    emit(format!("Git changes: {}", parts.join(", ")));
                }
            }
            emit("");
            let hl = self.highlighted(input, end);
            self.r.code_block(&hl, &plain, start, end, git, self.view);
            out.copied = plain[start - 1..end].join("\n");
            return Ok(out);
        }

        // --method -----------------------------------------------------------
        if !cli.methods.is_empty() {
            let a = analysis.as_ref().ok_or_else(|| {
                anyhow!(
                    "{}: no structure analysis for this file type, use -L or -c instead",
                    input.label
                )
            })?;
            let upto = cli
                .methods
                .iter()
                .flat_map(|q| a.find(q))
                .map(|i| a.items[i].end)
                .max()
                .unwrap_or(0);
            let hl = self.highlighted(input, upto);
            out.matched = false;
            for q in &cli.methods {
                let ids = a.find(q);
                if ids.is_empty() {
                    if !multi {
                        eprintln!(
                            "{} '{}' not found in {}",
                            self.r.icon_warn(),
                            q.bold(),
                            input.label
                        );
                        let s = a.suggest(q);
                        if !s.is_empty() {
                            eprintln!("   did you mean: {}?", s.join(", "));
                        }
                    }
                    continue;
                }
                out.matched = true;
                for id in ids {
                    let it = &a.items[id];
                    emit(self.r.item_header(a, it, git));
                    emit("");
                    self.r
                        .code_block(&hl, &plain, it.start, it.end, git, self.view);
                    emit("");
                    if !out.copied.is_empty() {
                        out.copied.push_str("\n\n");
                    }
                    out.copied
                        .push_str(&plain[it.start - 1..it.end.min(total)].join("\n"));
                }
            }
            return Ok(out);
        }

        // --code or unsupported language ---------------------------------------
        if cli.code || analysis.is_none() {
            if analysis.is_none() && !cli.code && !cli.duplicates {
                emit(format!(
                    "{} {}",
                    self.r.icon_info(),
                    "No structure analysis for this file type, showing the source (supported: py, rs, go, js)".dimmed()
                ));
            }
            if analysis.is_none() && cli.duplicates {
                bail!(
                    "{}: duplicate detection needs a supported language (py, rs, go, js)",
                    input.label
                );
            }
            if let Some(g) = git {
                self.r.git_summary(g);
            }
            let lang = input
                .lang
                .map(|l| format!(" │ {}", l.name()))
                .unwrap_or_default();
            emit(format!(
                "{}{} │ {} lines{}",
                if self.r.emoji { "📖 " } else { "" },
                input.label.bold().cyan(),
                total,
                lang
            ));
            emit("");
            let hl = self.highlighted(input, usize::MAX);
            self.r.code_block(&hl, &plain, 1, total, git, self.view);
            out.copied = input.text.clone();
            return Ok(out);
        }

        let a = analysis.as_ref().unwrap();
        let dups = a.find_duplicates();
        out.redefs = dups.redefinitions.len();

        // --duplicates ---------------------------------------------------------
        if cli.duplicates {
            self.r.duplicates(a, &dups, true);
            return Ok(out);
        }

        // default: structure ---------------------------------------------------
        self.r.structure(a, &input.label, git, &dups);
        Ok(out)
    }

    fn save_mode(&self, text: &str) -> Result<()> {
        if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
            return Ok(());
        }
        let stdin = std::io::stdin();
        loop {
            emit(format!(
                "\n{} {} │ Enter filename or 'q'/'exit' to quit",
                self.r.icon_save(),
                "Save Mode".bold()
            ));
            print!("{} ", if self.r.emoji { "📝" } else { ">" });
            std::io::stdout().flush().ok();
            let mut line = String::new();
            if stdin.read_line(&mut line)? == 0 {
                break;
            }
            let name = line.trim();
            if name.is_empty() {
                continue;
            }
            if matches!(name.to_lowercase().as_str(), "q" | "quit" | "exit") {
                break;
            }
            let p = Path::new(name);
            if p.exists() {
                print!(
                    "{} '{}' exists, overwrite? [y/N] ",
                    self.r.icon_warn(),
                    name
                );
                std::io::stdout().flush().ok();
                let mut ans = String::new();
                stdin.read_line(&mut ans)?;
                if !ans.trim().eq_ignore_ascii_case("y") {
                    continue;
                }
            }
            if let Some(parent) = p.parent().filter(|x| !x.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("cannot create {}", parent.display()))?;
            }
            std::fs::write(p, text).with_context(|| format!("cannot write {}", p.display()))?;
            emit(format!(
                "{} {}",
                self.r.icon_ok(),
                format!("File saved successfully: {name}").green()
            ));
            break;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn supports_truecolor() -> bool {
    let has = |k: &str, needles: &[&str]| {
        std::env::var(k)
            .map(|v| needles.iter().any(|n| v.contains(n)))
            .unwrap_or(false)
    };
    has("COLORTERM", &["truecolor", "24bit"])
        || has("TERM", &["truecolor", "24bit", "direct"])
        || std::env::var_os("WT_SESSION").is_some()
        || std::env::var_os("TERM_PROGRAM").is_some()
        || cfg!(windows)
}

fn parse_range_token(t: &str) -> Option<(usize, Option<usize>)> {
    let t = t.trim();
    for sep in ['-', ':'] {
        if let Some((a, b)) = t.split_once(sep) {
            return Some((a.trim().parse().ok()?, Some(b.trim().parse().ok()?)));
        }
    }
    Some((t.parse().ok()?, None))
}

fn list_themes(themes: &Themes, ss: &SyntaxSet, color: bool, truecolor: bool, emoji: bool) {
    let names = themes.names();
    emit(format!(
        "{}{} ({})",
        if emoji { "🎨 " } else { "" },
        "Available themes".bold(),
        names.len()
    ));
    emit("");
    let w = names.iter().map(|n| n.chars().count()).max().unwrap_or(10);
    let sample = "def greet(n): return f\"hi {n}\"  # 42";
    let syntax = ss
        .find_syntax_by_token("py")
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    for n in &names {
        let Some(theme) = themes.get(n) else { continue };
        let kind = if Themes::is_dark(theme) {
            "dark "
        } else {
            "light"
        };
        let preview = if color {
            highlight::highlight_lines(sample, ss, syntax, theme, truecolor, usize::MAX)
                .pop()
                .unwrap_or_default()
        } else {
            String::new()
        };
        emit(format!(
            "  {:<w$}  {}  {}",
            n.bold(),
            kind.bright_black(),
            preview,
            w = w
        ));
    }
    emit(format!("\nUse: {}", "rsread FILE -s <name>".bold()));
}

fn run(mut cli: Cli) -> Result<i32> {
    use std::io::IsTerminal as _;
    let tty = std::io::stdout().is_terminal();
    let color = match cli.color {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => {
            tty && std::env::var_os("NO_COLOR").is_none()
                && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true)
        }
    };
    colored::control::set_override(color);
    #[cfg(windows)]
    if color {
        let _ = colored::control::set_virtual_terminal(true);
    }
    let truecolor = supports_truecolor();

    let themes = Themes::load();
    let ss = two_face::syntax::extra_newlines();
    let emoji = !cli.no_emoji;

    if cli.list_themes {
        list_themes(&themes, &ss, color, truecolor, emoji);
        return Ok(0);
    }

    // `-L 10 20 file.py`: a second token that is not a number is really a file argument
    let mut range = None;
    if !cli.lines.is_empty() {
        let toks = cli.lines.clone();
        let (start, mut end) = parse_range_token(&toks[0])
            .ok_or_else(|| anyhow!("invalid line number '{}'", toks[0]))?;
        if let Some(second) = toks.get(1) {
            match second.trim().parse::<usize>() {
                Ok(n) if end.is_none() => end = Some(n),
                _ => cli.files.push(second.clone()),
            }
        }
        range = Some((start, end));
    }

    if cli.files.is_empty() {
        if !std::io::stdin().is_terminal() {
            cli.files.push("-".into());
        } else {
            Cli::command().print_help()?;
            return Ok(2);
        }
    }

    let (theme_name, theme) = themes.resolve(&cli.style)?;
    let _ = theme_name;
    let width = if tty {
        terminal_size::terminal_size()
            .map(|(w, _)| w.0 as usize)
            .unwrap_or(100)
    } else {
        120
    };

    let app = App {
        cli: &cli,
        r: Renderer { emoji, width },
        ss,
        theme: theme.clone(),
        color,
        truecolor,
        view: View {
            line_numbers: !cli.no_linenumber,
            strip: cli.strip,
        },
        range,
    };

    let inputs = gather(&cli)?;
    let multi = inputs.len() > 1;
    let mut any_match = false;
    let mut redefs = 0;
    let mut copied = String::new();
    let mut jsons = Vec::new();
    for input in &inputs {
        let o = app.process(input, multi)?;
        any_match |= o.matched;
        redefs += o.redefs;
        if !o.copied.is_empty() {
            if !copied.is_empty() {
                copied.push_str("\n\n");
            }
            copied.push_str(&o.copied);
        }
        jsons.extend(o.json);
    }

    if cli.json {
        let v = if jsons.len() == 1 {
            jsons.remove(0)
        } else {
            serde_json::Value::Array(jsons)
        };
        emit(serde_json::to_string_pretty(&v)?);
    }

    if cli.copy {
        if copied.is_empty() {
            eprintln!(
                "{} nothing to copy (use -y with -m, -L or -c)",
                app.r.icon_warn()
            );
        } else {
            clip::set(&copied)?;
            emit(format!(
                "{} {}",
                app.r.icon_copy(),
                format!("Copied {} lines to the clipboard", copied.lines().count()).green()
            ));
        }
    }

    if let Some(i) = inputs.iter().find(|i| i.clipboard) {
        app.save_mode(&i.text)?;
    }

    if !cli.methods.is_empty() && !any_match {
        return Ok(1);
    }
    if cli.duplicates && redefs > 0 {
        return Ok(2);
    }
    Ok(0)
}

fn main() {
    let os_args: Vec<string> = std::env::args().collect();
    if os_args.len() == 2 && (os_args[1] == "-V" || os_args[1] == "--version") {
        let version = colorful_version!();
        version.print_and_exit();
    }
    // `-nl` is a multi-letter short flag in pyread; clap would read it as `-n -l`
    let args: Vec<std::ffi::OsString> = std::env::args_os()
        .map(|a| {
            if a == "-nl" {
                "--no-linenumber".into()
            } else {
                a
            }
        })
        .collect();
    let cli = Cli::parse_from(args);
    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("{} {e}", "❌".red());
            if std::env::var_os("TRACEBACK").is_some() {
                eprintln!("{e:?}");
            }
            std::process::exit(1);
        }
    }
}
