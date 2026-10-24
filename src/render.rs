//! Terminal rendering: structure tree, code blocks with Git markers, warning panels.

use crate::analyzer::{Analysis, Duplicates, Item, Kind};
use crate::git::{Change, GitInfo, ItemGit};
use crate::highlight::strip_visible_prefix;
use crate::lang::Lang;
use colored::{Color, Colorize};
use std::collections::HashSet;
use std::io::Write;
use unicode_width::UnicodeWidthStr;

/// Print a line to stdout; exit quietly on a closed pipe (`rsread x.py | head`).
pub fn emit(s: impl AsRef<str>) {
    let mut out = std::io::stdout().lock();
    if writeln!(out, "{}", s.as_ref()).is_err() {
        std::process::exit(0);
    }
}

pub struct Renderer {
    pub emoji: bool,
    pub width: usize,
}

#[derive(Clone, Copy)]
pub struct View {
    pub line_numbers: bool,
    pub strip: bool,
}

enum Entry<'a> {
    Item(&'a Item),
    Group(Vec<&'a Item>),
}

fn digits(n: usize) -> usize {
    n.max(1).to_string().len()
}

fn truncate_width(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

impl Renderer {
    fn ico(&self, emoji: &'static str, ascii: &'static str) -> &'static str {
        if self.emoji {
            emoji
        } else {
            ascii
        }
    }

    pub fn icon_info(&self) -> &'static str {
        self.ico("ℹ️ ", "[i]")
    }

    pub fn icon_warn(&self) -> &'static str {
        self.ico("⚠️ ", "[!]")
    }

    pub fn icon_ok(&self) -> &'static str {
        self.ico("✅", "[ok]")
    }

    pub fn icon_copy(&self) -> &'static str {
        self.ico("📋", "[copy]")
    }

    pub fn icon_dir(&self) -> &'static str {
        self.ico("📁", "[dir]")
    }

    pub fn icon_file(&self) -> &'static str {
        self.ico("📄", "[file]")
    }

    pub fn icon_save(&self) -> &'static str {
        self.ico("💾", "[save]")
    }

    fn kind_icon(&self, it: &Item) -> &'static str {
        match (it.kind, it.is_async) {
            (Kind::Class, _) => self.ico("🏛️", "C"),
            (Kind::Struct, _) => self.ico("🏗️", "S"),
            (Kind::Enum, _) => self.ico("🔢", "E"),
            (Kind::Trait, _) | (Kind::Interface, _) => self.ico("🧩", "T"),
            (Kind::Impl, _) => self.ico("🔩", "I"),
            (Kind::Module, _) => self.ico("📦", "M"),
            (Kind::Type, _) => self.ico("🏷️", "t"),
            (Kind::Macro, _) => self.ico("✨", "!"),
            (Kind::Function, true) | (Kind::Method, true) => self.ico("⚡", "~"),
            (Kind::Function, false) => self.ico("🔧", "f"),
            (Kind::Method, false) => self.ico("⚙️", "m"),
        }
    }

    fn name_styled(&self, it: &Item) -> String {
        let n = it.name.as_str();
        match it.kind {
            Kind::Class
            | Kind::Struct
            | Kind::Enum
            | Kind::Trait
            | Kind::Interface
            | Kind::Type => n.bold().cyan().to_string(),
            Kind::Impl => n.bold().blue().to_string(),
            Kind::Module | Kind::Macro => n.bold().magenta().to_string(),
            Kind::Function => n.bold().yellow().to_string(),
            Kind::Method => n.bold().green().to_string(),
        }
    }

    fn badge_parts(g: &ItemGit) -> Vec<(String, Color)> {
        let mut v = Vec::new();
        if g.added > 0 {
            v.push((format!("+{}", g.added), Color::Green));
        }
        if g.modified > 0 {
            v.push((format!("~{}", g.modified), Color::Yellow));
        }
        if g.deleted > 0 {
            v.push((format!("-{}", g.deleted), Color::Red));
        }
        v
    }

    pub fn git_badge(g: &ItemGit) -> String {
        Self::badge_parts(g)
            .into_iter()
            .map(|(t, c)| t.color(c).bold().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn git_badge_plain_width(g: &ItemGit) -> usize {
        let parts = Self::badge_parts(g);
        parts.iter().map(|(t, _)| t.len()).sum::<usize>() + parts.len().saturating_sub(1)
    }

    pub fn git_summary(&self, g: &GitInfo) {
        if !g.has_changes() {
            return;
        }
        emit(format!(
            "{}{}",
            self.ico("📝 ", ""),
            "Git Changes Summary".bold()
        ));
        if g.added > 0 {
            emit(format!("{} {} lines added", "+".green().bold(), g.added));
        }
        if g.deleted > 0 {
            emit(format!("{} {} lines deleted", "-".red().bold(), g.deleted));
        }
        if g.modified > 0 {
            emit(format!(
                "{} {} lines modified",
                "~".yellow().bold(),
                g.modified
            ));
        }
        emit("");
    }

    pub fn panel(&self, color: Color, lines: &[String]) {
        for l in lines {
            emit(format!("{} {}", "┃".color(color).bold(), l));
        }
    }

    // ---- structure ---------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn structure(&self, a: &Analysis, label: &str, git: Option<&GitInfo>, dups: &Duplicates) {
        if let Some(g) = git {
            self.git_summary(g);
        }
        let changed = git.is_some_and(|g| g.has_changes());
        let mut head = format!(
            "{}{} │ {}",
            self.ico("🌳 ", ""),
            format!("{} Code Analysis", a.lang.name()).bold(),
            label.bold().cyan()
        );
        if changed {
            head.push_str(&format!(
                " {}",
                format!("{}(Git changes detected)", self.ico("📝 ", "")).yellow()
            ));
        }
        emit(head);

        if !a.error_lines.is_empty() {
            let lines = a
                .error_lines
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            emit(format!(
                "{} {}",
                self.icon_warn(),
                format!("Syntax errors near line {lines}: structure may be incomplete").yellow()
            ));
        }

        let flagged: HashSet<usize> = dups
            .redefinitions
            .iter()
            .flat_map(|g| g.ids.iter().copied())
            .collect();

        let top = a.children(None);
        let funcs: Vec<&Item> = top
            .iter()
            .copied()
            .filter(|i| i.kind == Kind::Function)
            .collect();
        let others: Vec<&Item> = top
            .iter()
            .copied()
            .filter(|i| i.kind != Kind::Function)
            .collect();
        let mut entries: Vec<Entry> = others.into_iter().map(Entry::Item).collect();
        if !funcs.is_empty() {
            if entries.is_empty() {
                entries.extend(funcs.into_iter().map(Entry::Item));
            } else {
                entries.push(Entry::Group(funcs));
            }
        }
        if entries.is_empty() {
            emit(format!(
                "{} {}",
                self.icon_info(),
                "No classes, functions or methods found".dimmed()
            ));
        }
        let n = entries.len();
        for (i, e) in entries.iter().enumerate() {
            let last = i + 1 == n;
            match e {
                Entry::Item(it) => self.tree_item(a, it, "", last, git, &flagged),
                Entry::Group(items) => {
                    let conn = if last { "└── " } else { "├── " };
                    emit(format!(
                        "{}{}{}",
                        conn.bright_black(),
                        self.ico("📄 ", ""),
                        "Standalone Functions".bold().white()
                    ));
                    let prefix = if last { "    " } else { "│   " };
                    for (j, it) in items.iter().enumerate() {
                        self.tree_item(a, it, prefix, j + 1 == items.len(), git, &flagged);
                    }
                }
            }
        }

        self.stats_line(a);
        if !dups.redefinitions.is_empty() || !dups.identical.is_empty() {
            emit("");
            self.duplicates(a, dups, false);
        }
    }

    fn tree_item(
        &self,
        a: &Analysis,
        it: &Item,
        prefix: &str,
        last: bool,
        git: Option<&GitInfo>,
        flagged: &HashSet<usize>,
    ) {
        let conn = if last { "└── " } else { "├── " };
        emit(format!(
            "{}{}{}",
            prefix.bright_black(),
            conn.bright_black(),
            self.item_line(a, it, prefix.width(), git, flagged)
        ));
        let kids = a.children(Some(it.id));
        let child_prefix = format!("{}{}", prefix, if last { "    " } else { "│   " });
        let n = kids.len();
        for (i, k) in kids.iter().enumerate() {
            self.tree_item(a, k, &child_prefix, i + 1 == n, git, flagged);
        }
    }

    fn item_line(
        &self,
        a: &Analysis,
        it: &Item,
        prefix_w: usize,
        git: Option<&GitInfo>,
        flagged: &HashSet<usize>,
    ) -> String {
        let icon = self.kind_icon(it);
        let range = format!(":{}-{}", it.start, it.end);
        let g = git
            .map(|g| g.range_stats(it.start, it.end))
            .unwrap_or_default();
        let dup = flagged.contains(&it.id);

        let mut used = prefix_w + 4 + icon.width() + 1 + it.name.width() + 1 + range.width();
        if !g.is_empty() {
            used += 1 + Self::git_badge_plain_width(&g);
        }
        if dup {
            used += 3;
        }
        let budget = self.width.saturating_sub(used + 2);
        let tail = if !it.tail.is_empty() && budget >= 6 {
            truncate_width(&it.tail, budget)
        } else {
            String::new()
        };
        used += tail.width();

        let mut s = format!("{} {}", icon, self.name_styled(it));
        if !tail.is_empty() {
            s.push_str(&tail.bright_black().to_string());
        }
        s.push(' ');
        s.push_str(&range.bright_black().to_string());
        if !g.is_empty() {
            s.push(' ');
            s.push_str(&Self::git_badge(&g));
        }
        if dup {
            s.push_str(&format!(" {}", self.icon_warn().trim().red()));
        }
        if a.lang == Lang::Python && !it.decorators.is_empty() {
            let d = it
                .decorators
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            if self.width > used + d.width() + 4 {
                used += d.width() + 1;
                s.push_str(&format!(" {}", d.magenta().dimmed()));
            }
        }
        if let Some(sum) = &it.summary {
            let room = self.width.saturating_sub(used + 4);
            if room >= 12 {
                s.push_str(&format!(
                    " {}",
                    format!("— {}", truncate_width(sum, room))
                        .bright_black()
                        .italic()
                ));
            }
        }
        s
    }

    fn stats_line(&self, a: &Analysis) {
        let mut parts = vec![format!("{} lines ({} blank)", a.total_lines, a.blank_lines)];
        for k in Kind::ALL {
            let n = a.items.iter().filter(|i| i.kind == k).count();
            if n > 0 {
                parts.push(format!(
                    "{} {}",
                    n,
                    if n == 1 { singular(k) } else { k.plural() }
                ));
            }
        }
        emit(format!(
            "\n{}{}",
            self.ico("📊 ", ""),
            parts.join(" │ ").bright_black()
        ));
    }

    // ---- duplicates --------------------------------------------------------

    pub fn duplicates(&self, a: &Analysis, d: &Duplicates, show_shared: bool) {
        let mut shown = false;
        if !d.redefinitions.is_empty() {
            shown = true;
            let mut lines = vec![format!(
                "{}{}",
                self.ico("⚠️  ", "[!] "),
                "DUPLICATE METHODS/FUNCTIONS DETECTED".red().bold()
            )];
            for g in &d.redefinitions {
                lines.push(String::new());
                lines.push(format!(
                    "• {} defined {} times {}",
                    g.name.bold(),
                    g.ids.len(),
                    "(the last definition silently wins)".bright_black()
                ));
                self.dup_lines(a, &g.ids, &mut lines);
            }
            self.panel(Color::Red, &lines);
        }
        if !d.identical.is_empty() {
            if shown {
                emit("");
            }
            shown = true;
            let mut lines = vec![format!(
                "{}{}",
                self.ico("👯 ", "[=] "),
                "IDENTICAL BODIES (copy-paste candidates)".yellow().bold()
            )];
            for g in &d.identical {
                lines.push(String::new());
                lines.push(format!("• {}", g.name.bold()));
                self.dup_lines(a, &g.ids, &mut lines);
            }
            self.panel(Color::Yellow, &lines);
        }
        if show_shared && !d.shared_names.is_empty() {
            if shown {
                emit("");
            }
            shown = true;
            let mut lines = vec![format!(
                "{}{}",
                self.ico("🔁 ", "[~] "),
                "SAME NAME IN MULTIPLE SCOPES".blue().bold()
            )];
            for g in &d.shared_names {
                lines.push(String::new());
                lines.push(format!("• {} found {} times:", g.name.bold(), g.ids.len()));
                self.dup_lines(a, &g.ids, &mut lines);
            }
            self.panel(Color::Blue, &lines);
        }
        if !shown {
            emit(format!(
                "{} {}",
                self.icon_ok(),
                "No duplicate methods/functions found".green()
            ));
        }
    }

    fn dup_lines(&self, a: &Analysis, ids: &[usize], out: &mut Vec<String>) {
        let n = ids.len();
        for (i, id) in ids.iter().enumerate() {
            let it = &a.items[*id];
            let mut ctx = a.context(it);
            if it.variant {
                ctx.push_str(", conditional/variant");
            }
            let conn = if i + 1 == n { "└─" } else { "├─" };
            out.push(format!(
                "  {} Line {} {}",
                conn,
                it.def_line,
                format!("({ctx})").bright_black()
            ));
        }
    }

    // ---- code --------------------------------------------------------------

    /// Print lines `start..=end` (1-based). `hl` holds one display string per line.
    pub fn code_block(
        &self,
        hl: &[String],
        plain: &[&str],
        start: usize,
        end: usize,
        git: Option<&GitInfo>,
        view: View,
    ) {
        let end = end.min(hl.len());
        if start == 0 || start > end {
            return;
        }
        let marker_col = git.is_some_and(|g| g.has_changes());
        let w = digits(end);
        let indent = if view.strip {
            plain[start - 1..end]
                .iter()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').count())
                .min()
                .unwrap_or(0)
        } else {
            0
        };
        for ln in start..=end {
            let change = git.and_then(|g| g.changes.get(&ln)).copied();
            let (mark, color) = match change {
                Some(Change::Added) => ("+", Some(Color::Green)),
                Some(Change::Modified) => ("~", Some(Color::Yellow)),
                Some(Change::DeletedBelow) => ("-", Some(Color::Red)),
                None => (" ", None),
            };
            let mut s = String::new();
            if !view.strip {
                s.push_str("  ");
            }
            if view.line_numbers {
                let num = format!("{ln:>w$}");
                match color {
                    Some(c) => s.push_str(&num.color(c).bold().to_string()),
                    None => s.push_str(&num.bright_black().to_string()),
                }
                s.push_str(&" │ ".bright_black().to_string());
            }
            if marker_col {
                match color {
                    Some(c) => s.push_str(&mark.color(c).bold().to_string()),
                    None => s.push(' '),
                }
                s.push(' ');
            }
            let code = &hl[ln - 1];
            if indent > 0 {
                s.push_str(&strip_visible_prefix(code, indent));
            } else {
                s.push_str(code);
            }
            emit(s);
        }
    }

    pub fn item_header(&self, a: &Analysis, it: &Item, git: Option<&GitInfo>) -> String {
        let label = match (it.kind, it.parent) {
            (Kind::Method, Some(p)) => format!("{} Method", a.items[p].kind.label()),
            (k, _) => k.label().to_string(),
        };
        let mut s = format!(
            "{}{} '{}' │ Lines {}-{}",
            self.kind_icon(it).to_string() + " ",
            label.bold(),
            it.qual.bold().cyan(),
            it.start,
            it.end
        );
        if let Some(g) = git {
            let st = g.range_stats(it.start, it.end);
            if !st.is_empty() {
                s.push_str(&format!(" │ Git: {}", Self::git_badge(&st)));
            }
        }
        s
    }
}

fn singular(k: Kind) -> &'static str {
    match k {
        Kind::Class => "class",
        Kind::Struct => "struct",
        Kind::Enum => "enum",
        Kind::Trait => "trait",
        Kind::Interface => "interface",
        Kind::Impl => "impl block",
        Kind::Module => "module",
        Kind::Type => "type",
        Kind::Macro => "macro",
        Kind::Function => "function",
        Kind::Method => "method",
    }
}
