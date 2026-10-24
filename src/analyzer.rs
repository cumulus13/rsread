//! Structure analysis on top of tree-sitter: classes, structs, impls, methods,
//! functions, plus duplicate detection. Error tolerant: broken code still yields
//! a (partial) structure instead of failing like a strict AST parser would.

use crate::lang::Lang;
use anyhow::{anyhow, Result};
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Impl,
    Module,
    Type,
    Macro,
    Function,
    Method,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Class => "Class",
            Kind::Struct => "Struct",
            Kind::Enum => "Enum",
            Kind::Trait => "Trait",
            Kind::Interface => "Interface",
            Kind::Impl => "Impl",
            Kind::Module => "Module",
            Kind::Type => "Type",
            Kind::Macro => "Macro",
            Kind::Function => "Function",
            Kind::Method => "Method",
        }
    }

    pub fn plural(self) -> &'static str {
        match self {
            Kind::Class => "classes",
            Kind::Struct => "structs",
            Kind::Enum => "enums",
            Kind::Trait => "traits",
            Kind::Interface => "interfaces",
            Kind::Impl => "impl blocks",
            Kind::Module => "modules",
            Kind::Type => "types",
            Kind::Macro => "macros",
            Kind::Function => "functions",
            Kind::Method => "methods",
        }
    }

    pub fn is_callable(self) -> bool {
        matches!(self, Kind::Function | Kind::Method)
    }

    pub const ALL: [Kind; 11] = [
        Kind::Module,
        Kind::Class,
        Kind::Struct,
        Kind::Enum,
        Kind::Trait,
        Kind::Interface,
        Kind::Type,
        Kind::Impl,
        Kind::Macro,
        Kind::Function,
        Kind::Method,
    ];
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub id: usize,
    pub kind: Kind,
    pub name: String,
    /// Lookup name, e.g. `MyClass.method` (`Type.method` for Rust impls too).
    pub qual: String,
    pub parent: Option<usize>,
    /// First line (1-based) including decorators / attributes / doc comments.
    pub start: usize,
    /// Last line (1-based, inclusive).
    pub end: usize,
    /// Line of the definition keyword itself.
    pub def_line: usize,
    /// Text after the name up to the body: parameters, return type, bases...
    pub tail: String,
    pub decorators: Vec<String>,
    #[serde(rename = "async")]
    pub is_async: bool,
    /// Intentional alternate definition (conditional block, cfg, overload, setter...).
    pub variant: bool,
    pub summary: Option<String>,
    pub depth: usize,
    #[serde(skip)]
    pub member_prefix: String,
    #[serde(skip)]
    pub child_scope: String,
    #[serde(skip)]
    pub scope: String,
    #[serde(skip)]
    pub body_norm: Option<String>,
}

#[derive(Debug)]
pub struct Analysis {
    pub lang: Lang,
    pub items: Vec<Item>,
    pub total_lines: usize,
    pub blank_lines: usize,
    pub error_lines: Vec<usize>,
    /// children[i] = ids of the children of item i, sorted by line; `roots` for top level.
    children: Vec<Vec<usize>>,
    roots: Vec<usize>,
}

#[derive(Debug, Default)]
pub struct DupGroup {
    pub name: String,
    pub ids: Vec<usize>,
}

#[derive(Debug, Default)]
pub struct Duplicates {
    /// Same name defined twice in the same scope: the later one silently wins.
    pub redefinitions: Vec<DupGroup>,
    /// Different names, identical body: copy-paste candidates.
    pub identical: Vec<DupGroup>,
    /// Same method name in several classes/types (informational).
    pub shared_names: Vec<DupGroup>,
}

impl Analysis {
    pub fn children(&self, parent: Option<usize>) -> Vec<&Item> {
        let ids = match parent {
            Some(p) => &self.children[p],
            None => &self.roots,
        };
        ids.iter().map(|i| &self.items[*i]).collect()
    }

    pub fn context(&self, it: &Item) -> String {
        match it.parent {
            Some(p) => {
                let p = &self.items[p];
                format!("in {} {}", p.kind.label().to_lowercase(), p.name)
            }
            None => "standalone".to_string(),
        }
    }

    /// Resolve a `-m` query: exact qualified name, simple name, qualified suffix,
    /// then case-insensitive; `re:<pattern>` matches qualified names by regex.
    pub fn find(&self, query: &str) -> Vec<usize> {
        if let Some(pat) = query.strip_prefix("re:") {
            return match Regex::new(pat) {
                Ok(re) => self
                    .items
                    .iter()
                    .filter(|i| re.is_match(&i.qual))
                    .map(|i| i.id)
                    .collect(),
                Err(_) => vec![],
            };
        }
        let q = query.replace("::", ".");
        let q = q.trim();
        let pick = |f: &dyn Fn(&Item) -> bool| -> Vec<usize> {
            self.items.iter().filter(|i| f(i)).map(|i| i.id).collect()
        };
        let r = pick(&|i| i.qual == q);
        if !r.is_empty() {
            return r;
        }
        let r = pick(&|i| i.name == q);
        if !r.is_empty() {
            return r;
        }
        let suffix = format!(".{q}");
        let r = pick(&|i| i.qual.ends_with(&suffix));
        if !r.is_empty() {
            return r;
        }
        let ql = q.to_lowercase();
        pick(&|i| i.qual.to_lowercase() == ql || i.name.to_lowercase() == ql)
    }

    pub fn suggest(&self, query: &str) -> Vec<String> {
        let q = query.replace("::", ".").to_lowercase();
        let mut seen = HashSet::new();
        let mut scored: Vec<(usize, String)> = Vec::new();
        for it in &self.items {
            let name = it.name.to_lowercase();
            let qual = it.qual.to_lowercase();
            let d = levenshtein(&name, &q);
            let hit = (!q.is_empty() && (qual.contains(&q) || name.contains(&q))) || d <= 2;
            if hit && seen.insert(it.qual.clone()) {
                scored.push((d, it.qual.clone()));
            }
        }
        scored.sort();
        scored.into_iter().take(5).map(|(_, q)| q).collect()
    }

    pub fn find_duplicates(&self) -> Duplicates {
        let mut dups = Duplicates::default();

        // 1. redefinitions in the same scope
        let mut groups: BTreeMap<(String, String, u8), Vec<usize>> = BTreeMap::new();
        for it in &self.items {
            if it.kind == Kind::Impl {
                continue;
            }
            let cat = match it.kind {
                Kind::Function | Kind::Method => 0,
                Kind::Macro => 2,
                _ => 1,
            };
            groups
                .entry((it.scope.clone(), it.name.clone(), cat))
                .or_default()
                .push(it.id);
        }
        for ((_, name, _), ids) in &groups {
            let real = ids.iter().filter(|i| !self.items[**i].variant).count();
            if ids.len() > 1 && real >= 2 {
                let first = &self.items[ids[0]];
                let label = if first.qual.is_empty() {
                    name.clone()
                } else {
                    first.qual.clone()
                };
                dups.redefinitions.push(DupGroup {
                    name: label,
                    ids: sorted_by_line(self, ids),
                });
            }
        }

        // 2. identical bodies
        let mut bodies: HashMap<&str, Vec<usize>> = HashMap::new();
        for it in &self.items {
            if let Some(b) = &it.body_norm {
                if b.len() >= 60 && b.lines().count() >= 3 {
                    bodies.entry(b.as_str()).or_default().push(it.id);
                }
            }
        }
        for ids in bodies.values() {
            if ids.len() > 1 {
                let ids = sorted_by_line(self, ids);
                let name = ids
                    .iter()
                    .map(|i| self.items[*i].qual.clone())
                    .collect::<Vec<_>>()
                    .join(" = ");
                dups.identical.push(DupGroup { name, ids });
            }
        }

        // 3. same method name in different scopes
        const COMMON: [&str; 8] = [
            "constructor",
            "new",
            "default",
            "main",
            "init",
            "setUp",
            "tearDown",
            "run_tests",
        ];
        let mut by_name: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for it in &self.items {
            if it.kind != Kind::Method {
                continue;
            }
            let dunder = it.name.starts_with("__") && it.name.ends_with("__");
            let trait_impl = it.scope.starts_with("impl ") && it.scope.contains(" for ");
            if dunder || trait_impl || COMMON.contains(&it.name.as_str()) {
                continue;
            }
            by_name.entry(it.name.as_str()).or_default().push(it.id);
        }
        for (name, ids) in by_name {
            let scopes: HashSet<&str> = ids.iter().map(|i| self.items[*i].scope.as_str()).collect();
            if scopes.len() > 1 {
                dups.shared_names.push(DupGroup {
                    name: name.to_string(),
                    ids: sorted_by_line(self, &ids),
                });
            }
        }

        dups.redefinitions
            .sort_by_key(|g| self.items[g.ids[0]].start);
        dups.identical.sort_by_key(|g| self.items[g.ids[0]].start);
        dups
    }
}

fn sorted_by_line(a: &Analysis, ids: &[usize]) -> Vec<usize> {
    let mut v = ids.to_vec();
    v.sort_by_key(|i| a.items[*i].start);
    v
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

// ---------------------------------------------------------------------------
// tree-sitter walking
// ---------------------------------------------------------------------------

fn named_kids<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}

fn first_row(n: Node) -> usize {
    n.start_position().row + 1
}

fn last_row(n: Node) -> usize {
    let e = n.end_position();
    if e.column == 0 && e.row > n.start_position().row {
        e.row
    } else {
        e.row + 1
    }
}

fn is_async(n: Node, src: &str) -> bool {
    let mut c = n.walk();
    let found = n.children(&mut c).any(|ch| {
        ch.kind() == "async"
            || (ch.kind() == "function_modifiers"
                && ch
                    .utf8_text(src.as_bytes())
                    .map(|t| t.contains("async"))
                    .unwrap_or(false))
    });
    found
}

/// Contiguous run of leading nodes (attributes / comments) directly above `target_row` (0-based).
fn lead_chain<'t>(pending: &[Node<'t>], target_row: usize) -> Vec<Node<'t>> {
    let mut out = Vec::new();
    let mut expect = target_row;
    for n in pending.iter().rev() {
        if last_row(*n) >= expect {
            out.push(*n);
            expect = n.start_position().row;
        } else {
            break;
        }
    }
    out.reverse();
    out
}

struct NewItem<'t> {
    kind: Kind,
    name: String,
    node: Node<'t>,
    outer: Node<'t>,
    name_end: usize,
    body: Option<Node<'t>>,
    lead: Vec<Node<'t>>,
    decorators: Vec<String>,
    variant: bool,
    summary: Option<String>,
    parent: Option<usize>,
}

struct Builder<'a> {
    src: &'a str,
    lang: Lang,
    items: Vec<Item>,
    go_methods: Vec<(usize, String)>,
}

const PY_COMPOUND: [&str; 12] = [
    "if_statement",
    "elif_clause",
    "else_clause",
    "try_statement",
    "except_clause",
    "except_group_clause",
    "finally_clause",
    "with_statement",
    "for_statement",
    "while_statement",
    "match_statement",
    "block",
];

impl<'a> Builder<'a> {
    fn text<'t>(&self, n: Node<'t>) -> &'a str {
        n.utf8_text(self.src.as_bytes()).unwrap_or("")
    }

    fn first_line(&self, n: Node) -> String {
        self.text(n).lines().next().unwrap_or("").trim().to_string()
    }

    fn tail(&self, node: Node, name_end: usize, body: Option<Node>) -> String {
        let end = body
            .map(|b| b.start_byte())
            .unwrap_or_else(|| node.end_byte());
        if end <= name_end {
            return String::new();
        }
        let raw = self.src.get(name_end..end).unwrap_or("");
        let mut s = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        loop {
            let t = s.trim_end_matches([':', '{', ';', ' ']).to_string();
            let t = t
                .strip_suffix("=>")
                .map(|x| x.trim_end().to_string())
                .unwrap_or(t);
            if t == s {
                break;
            }
            s = t;
        }
        s
    }

    fn clean_comment(&self, n: Node) -> Option<String> {
        for line in self.text(n).lines() {
            let l = line
                .trim()
                .trim_start_matches("///")
                .trim_start_matches("//!")
                .trim_start_matches("//")
                .trim_start_matches("/**")
                .trim_start_matches("/*")
                .trim_end_matches("*/")
                .trim_start_matches('*')
                .trim();
            if !l.is_empty() {
                return Some(l.to_string());
            }
        }
        None
    }

    fn py_docstring(&self, body: Node) -> Option<String> {
        let first = named_kids(body).into_iter().next()?;
        if first.kind() != "expression_statement" {
            return None;
        }
        let s = named_kids(first).into_iter().next()?;
        if s.kind() != "string" {
            return None;
        }
        let t = self.text(s);
        let t = t.trim_start_matches(|c: char| c.is_ascii_alphabetic());
        let t = t.trim_matches(|c| c == '"' || c == '\'');
        t.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|l| l.to_string())
    }

    fn push(&mut self, n: NewItem) -> usize {
        let id = self.items.len();
        let (prefix, scope) = match n.parent {
            Some(p) => (
                self.items[p].member_prefix.clone(),
                self.items[p].child_scope.clone(),
            ),
            None => (String::new(), String::new()),
        };
        let qual = if prefix.is_empty() {
            n.name.clone()
        } else {
            format!("{}.{}", prefix, n.name)
        };
        let mut start = first_row(n.outer);
        if let Some(l) = n.lead.first() {
            start = start.min(first_row(*l));
        }
        let body_norm = if n.kind.is_callable() {
            n.body.map(|b| {
                let t = self
                    .src
                    .get(b.start_byte()..n.node.end_byte())
                    .unwrap_or("");
                t.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        } else {
            None
        };
        self.items.push(Item {
            id,
            kind: n.kind,
            name: n.name,
            qual: qual.clone(),
            parent: n.parent,
            start,
            end: last_row(n.outer),
            def_line: first_row(n.node),
            tail: self.tail(n.node, n.name_end, n.body),
            decorators: n.decorators,
            is_async: is_async(n.node, self.src),
            variant: n.variant,
            summary: n.summary,
            depth: 0,
            member_prefix: qual.clone(),
            child_scope: qual,
            scope,
            body_norm,
        });
        id
    }

    fn parent_kind(&self, parent: Option<usize>) -> Option<Kind> {
        parent.map(|p| self.items[p].kind)
    }

    // ---- Python ----------------------------------------------------------

    fn py_walk<'t>(&mut self, node: Node<'t>, parent: Option<usize>, cond: bool) {
        for ch in named_kids(node) {
            match ch.kind() {
                "decorated_definition" => {
                    let Some(def) = ch.child_by_field_name("definition") else {
                        continue;
                    };
                    let decs: Vec<String> = named_kids(ch)
                        .into_iter()
                        .filter(|c| c.kind() == "decorator")
                        .map(|c| self.first_line(c))
                        .collect();
                    self.py_def(def, ch, decs, parent, cond);
                }
                "class_definition" | "function_definition" => {
                    self.py_def(ch, ch, vec![], parent, cond)
                }
                k if PY_COMPOUND.contains(&k) => self.py_walk(ch, parent, true),
                _ => {}
            }
        }
    }

    fn py_def<'t>(
        &mut self,
        def: Node<'t>,
        outer: Node<'t>,
        decs: Vec<String>,
        parent: Option<usize>,
        cond: bool,
    ) {
        let Some(name_n) = def.child_by_field_name("name") else {
            return;
        };
        let name = self.text(name_n).to_string();
        let is_class = def.kind() == "class_definition";
        let kind = if is_class {
            Kind::Class
        } else if self.parent_kind(parent) == Some(Kind::Class) {
            Kind::Method
        } else {
            Kind::Function
        };
        let body = def.child_by_field_name("body");
        let variant = cond
            || name == "_"
            || decs.iter().any(|d| {
                d.contains(".setter")
                    || d.contains(".getter")
                    || d.contains(".deleter")
                    || d.contains("overload")
                    || d.contains(".register")
            });
        let summary = body.and_then(|b| self.py_docstring(b));
        let id = self.push(NewItem {
            kind,
            name,
            node: def,
            outer,
            name_end: name_n.end_byte(),
            body,
            lead: vec![],
            decorators: decs,
            variant,
            summary,
            parent,
        });
        if is_class {
            if let Some(b) = body {
                self.py_walk(b, Some(id), false);
            }
        }
    }

    // ---- Rust ------------------------------------------------------------

    fn rs_walk<'t>(&mut self, node: Node<'t>, parent: Option<usize>) {
        let mut pending: Vec<Node<'t>> = Vec::new();
        for ch in named_kids(node) {
            let k = ch.kind();
            let txt = self.text(ch);
            let leading = k == "attribute_item"
                || (k == "line_comment" && txt.starts_with("///") && !txt.starts_with("////"))
                || (k == "block_comment" && txt.starts_with("/**") && !txt.starts_with("/**/"));
            if leading {
                pending.push(ch);
                continue;
            }
            let lead = lead_chain(&pending, ch.start_position().row);
            pending.clear();
            let decorators: Vec<String> = lead
                .iter()
                .filter(|n| n.kind() == "attribute_item")
                .map(|n| self.first_line(*n))
                .collect();
            let summary = lead
                .iter()
                .filter(|n| n.kind() != "attribute_item")
                .find_map(|n| self.clean_comment(*n));
            let variant = decorators.iter().any(|d| d.starts_with("#[cfg"));
            let name_field = |n: Node<'t>| n.child_by_field_name("name");
            let pk = self.parent_kind(parent);

            let (kind, name_node) = match k {
                "function_item" | "function_signature_item" => (
                    if matches!(pk, Some(Kind::Impl) | Some(Kind::Trait)) {
                        Kind::Method
                    } else {
                        Kind::Function
                    },
                    name_field(ch),
                ),
                "struct_item" | "union_item" => (Kind::Struct, name_field(ch)),
                "enum_item" => (Kind::Enum, name_field(ch)),
                "trait_item" => (Kind::Trait, name_field(ch)),
                "type_item" => (Kind::Type, name_field(ch)),
                "mod_item" => (Kind::Module, name_field(ch)),
                "macro_definition" => (Kind::Macro, name_field(ch)),
                "impl_item" => {
                    self.rs_impl(ch, lead, decorators, summary, variant, parent);
                    continue;
                }
                _ => continue,
            };
            let Some(nn) = name_node else { continue };
            let body = ch.child_by_field_name("body");
            let id = self.push(NewItem {
                kind,
                name: self.text(nn).to_string(),
                node: ch,
                outer: ch,
                name_end: nn.end_byte(),
                body,
                lead,
                decorators,
                variant,
                summary,
                parent,
            });
            if matches!(kind, Kind::Trait | Kind::Module) {
                if let Some(b) = body {
                    self.rs_walk(b, Some(id));
                }
            }
        }
    }

    fn rs_impl<'t>(
        &mut self,
        node: Node<'t>,
        lead: Vec<Node<'t>>,
        decorators: Vec<String>,
        summary: Option<String>,
        variant: bool,
        parent: Option<usize>,
    ) {
        let Some(ty) = node.child_by_field_name("type") else {
            return;
        };
        let ty_text = self
            .text(ty)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let base = ty_text
            .split('<')
            .next()
            .unwrap_or(&ty_text)
            .trim()
            .to_string();
        let trait_text = node.child_by_field_name("trait").map(|t| {
            self.text(t)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        });
        let label = match &trait_text {
            Some(t) => format!("impl {t} for {ty_text}"),
            None => format!("impl {ty_text}"),
        };
        let body = node.child_by_field_name("body");
        let id = self.push(NewItem {
            kind: Kind::Impl,
            name: label.clone(),
            node,
            outer: node,
            name_end: ty.end_byte(),
            body,
            lead,
            decorators,
            variant,
            summary,
            parent,
        });
        self.items[id].tail = String::new();
        self.items[id].qual = label.clone();
        self.items[id].member_prefix = base;
        self.items[id].child_scope = label;
        if let Some(b) = body {
            self.rs_walk(b, Some(id));
        }
    }

    // ---- Go --------------------------------------------------------------

    fn go_walk<'t>(&mut self, node: Node<'t>) {
        let mut pending: Vec<Node<'t>> = Vec::new();
        for ch in named_kids(node) {
            if ch.kind() == "comment" {
                pending.push(ch);
                continue;
            }
            let lead = lead_chain(&pending, ch.start_position().row);
            pending.clear();
            let summary = lead.iter().find_map(|n| self.clean_comment(*n));
            match ch.kind() {
                "function_declaration" => {
                    let Some(nn) = ch.child_by_field_name("name") else {
                        continue;
                    };
                    let name = self.text(nn).to_string();
                    let variant = name == "init" || name == "_";
                    self.push(NewItem {
                        kind: Kind::Function,
                        name,
                        node: ch,
                        outer: ch,
                        name_end: nn.end_byte(),
                        body: ch.child_by_field_name("body"),
                        lead,
                        decorators: vec![],
                        variant,
                        summary,
                        parent: None,
                    });
                }
                "method_declaration" => {
                    let Some(nn) = ch.child_by_field_name("name") else {
                        continue;
                    };
                    let recv = ch
                        .child_by_field_name("receiver")
                        .and_then(|r| find_kind(r, "type_identifier"))
                        .map(|t| self.text(t).to_string())
                        .unwrap_or_default();
                    let id = self.push(NewItem {
                        kind: Kind::Method,
                        name: self.text(nn).to_string(),
                        node: ch,
                        outer: ch,
                        name_end: nn.end_byte(),
                        body: ch.child_by_field_name("body"),
                        lead,
                        decorators: vec![],
                        variant: false,
                        summary,
                        parent: None,
                    });
                    self.items[id].qual = if recv.is_empty() {
                        self.items[id].name.clone()
                    } else {
                        format!("{recv}.{}", self.items[id].name)
                    };
                    self.items[id].scope = recv.clone();
                    self.go_methods.push((id, recv));
                }
                "type_declaration" => {
                    let specs: Vec<Node<'t>> = named_kids(ch)
                        .into_iter()
                        .filter(|c| c.kind() == "type_spec" || c.kind() == "type_alias")
                        .collect();
                    let single = specs.len() == 1;
                    for spec in specs {
                        let Some(nn) = spec.child_by_field_name("name") else {
                            continue;
                        };
                        let kind = match spec.child_by_field_name("type").map(|t| t.kind()) {
                            Some("struct_type") => Kind::Struct,
                            Some("interface_type") => Kind::Interface,
                            _ => Kind::Type,
                        };
                        self.push(NewItem {
                            kind,
                            name: self.text(nn).to_string(),
                            node: spec,
                            outer: if single { ch } else { spec },
                            name_end: nn.end_byte(),
                            body: spec
                                .child_by_field_name("type")
                                .filter(|_| kind != Kind::Type),
                            lead: if single { lead.clone() } else { vec![] },
                            decorators: vec![],
                            variant: false,
                            summary: if single { summary.clone() } else { None },
                            parent: None,
                        });
                    }
                }
                _ => {}
            }
        }
        // attach methods to their receiver type when it is declared in this file
        let types: HashMap<String, usize> = self
            .items
            .iter()
            .filter(|i| matches!(i.kind, Kind::Struct | Kind::Interface | Kind::Type))
            .map(|i| (i.name.clone(), i.id))
            .collect();
        for (id, recv) in std::mem::take(&mut self.go_methods) {
            if let Some(&pid) = types.get(&recv) {
                self.items[id].parent = Some(pid);
            }
        }
    }

    // ---- JavaScript ------------------------------------------------------

    fn js_walk<'t>(&mut self, node: Node<'t>) {
        let mut pending: Vec<Node<'t>> = Vec::new();
        for ch in named_kids(node) {
            if ch.kind() == "comment" {
                pending.push(ch);
                continue;
            }
            let lead = lead_chain(&pending, ch.start_position().row);
            pending.clear();
            let summary = lead.iter().find_map(|n| self.clean_comment(*n));
            self.js_stmt(ch, ch, lead, summary);
        }
    }

    fn js_stmt<'t>(
        &mut self,
        node: Node<'t>,
        outer: Node<'t>,
        lead: Vec<Node<'t>>,
        summary: Option<String>,
    ) {
        match node.kind() {
            "export_statement" => {
                if let Some(d) = node.child_by_field_name("declaration") {
                    self.js_stmt(d, node, lead, summary);
                }
            }
            "function_declaration" | "generator_function_declaration" => {
                let Some(nn) = node.child_by_field_name("name") else {
                    return;
                };
                self.push(NewItem {
                    kind: Kind::Function,
                    name: self.text(nn).to_string(),
                    node,
                    outer,
                    name_end: nn.end_byte(),
                    body: node.child_by_field_name("body"),
                    lead,
                    decorators: vec![],
                    variant: false,
                    summary,
                    parent: None,
                });
            }
            "class_declaration" => {
                let Some(nn) = node.child_by_field_name("name") else {
                    return;
                };
                let body = node.child_by_field_name("body");
                let id = self.push(NewItem {
                    kind: Kind::Class,
                    name: self.text(nn).to_string(),
                    node,
                    outer,
                    name_end: nn.end_byte(),
                    body,
                    lead,
                    decorators: vec![],
                    variant: false,
                    summary,
                    parent: None,
                });
                if let Some(b) = body {
                    for m in named_kids(b) {
                        if m.kind() != "method_definition" {
                            continue;
                        }
                        let Some(mn) = m.child_by_field_name("name") else {
                            continue;
                        };
                        let mut c = m.walk();
                        let accessor = m
                            .children(&mut c)
                            .any(|x| x.kind() == "get" || x.kind() == "set");
                        drop(c);
                        self.push(NewItem {
                            kind: Kind::Method,
                            name: self.text(mn).to_string(),
                            node: m,
                            outer: m,
                            name_end: mn.end_byte(),
                            body: m.child_by_field_name("body"),
                            lead: vec![],
                            decorators: vec![],
                            variant: accessor,
                            summary: None,
                            parent: Some(id),
                        });
                    }
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                for d in named_kids(node) {
                    if d.kind() != "variable_declarator" {
                        continue;
                    }
                    let (Some(nn), Some(v)) = (
                        d.child_by_field_name("name"),
                        d.child_by_field_name("value"),
                    ) else {
                        continue;
                    };
                    if !matches!(
                        v.kind(),
                        "arrow_function"
                            | "function_expression"
                            | "function"
                            | "generator_function"
                    ) {
                        continue;
                    }
                    self.push(NewItem {
                        kind: Kind::Function,
                        name: self.text(nn).to_string(),
                        node: v,
                        outer,
                        name_end: v.start_byte(),
                        body: v.child_by_field_name("body"),
                        lead: lead.clone(),
                        decorators: vec![],
                        variant: false,
                        summary: summary.clone(),
                        parent: None,
                    });
                }
            }
            _ => {}
        }
    }
}

fn find_kind<'t>(n: Node<'t>, kind: &str) -> Option<Node<'t>> {
    if n.kind() == kind {
        return Some(n);
    }
    named_kids(n).into_iter().find_map(|c| find_kind(c, kind))
}

fn collect_errors(node: Node, out: &mut Vec<usize>) {
    if out.len() >= 5 {
        return;
    }
    let mut c = node.walk();
    for ch in node.children(&mut c) {
        if ch.is_error() || ch.is_missing() {
            out.push(first_row(ch));
        } else if ch.has_error() {
            collect_errors(ch, out);
        }
    }
}

pub fn analyze(text: &str, lang: Lang) -> Result<Analysis> {
    let mut parser = Parser::new();
    let language: tree_sitter::Language = match lang {
        Lang::Python => tree_sitter_python::LANGUAGE.into(),
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
        Lang::Go => tree_sitter_go::LANGUAGE.into(),
        Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
    };
    parser
        .set_language(&language)
        .map_err(|e| anyhow!("failed to load {} grammar: {e}", lang.name()))?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| anyhow!("parser gave up on this input"))?;
    let root = tree.root_node();

    let mut b = Builder {
        src: text,
        lang,
        items: Vec::new(),
        go_methods: Vec::new(),
    };
    match b.lang {
        Lang::Python => b.py_walk(root, None, false),
        Lang::Rust => b.rs_walk(root, None),
        Lang::Go => b.go_walk(root),
        Lang::JavaScript => b.js_walk(root),
    }
    let mut items = b.items;

    // depth (parents may come after children for Go methods, so walk chains)
    let parents: Vec<Option<usize>> = items.iter().map(|i| i.parent).collect();
    for it in items.iter_mut() {
        let mut d = 0;
        let mut p = it.parent;
        while let Some(pi) = p {
            d += 1;
            p = parents[pi];
            if d > 64 {
                break;
            }
        }
        it.depth = d;
    }

    let mut error_lines = Vec::new();
    if root.has_error() {
        collect_errors(root, &mut error_lines);
    }
    let total_lines = text.lines().count();
    let blank_lines = text.lines().filter(|l| l.trim().is_empty()).count();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
    let mut roots = Vec::new();
    for it in &items {
        match it.parent {
            Some(p) => children[p].push(it.id),
            None => roots.push(it.id),
        }
    }
    let key = |i: &usize| (items[*i].start, *i);
    roots.sort_by_key(key);
    for c in children.iter_mut() {
        c.sort_by_key(key);
    }
    Ok(Analysis {
        lang,
        items,
        total_lines,
        blank_lines,
        error_lines,
        children,
        roots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(a: &Analysis) -> Vec<String> {
        a.items
            .iter()
            .map(|i| format!("{}:{}", i.kind.label(), i.qual))
            .collect()
    }

    #[test]
    fn python_structure_and_duplicates() {
        let src = r#"
import os

class A:
    """Doc of A."""
    @property
    def x(self): return 1
    @x.setter
    def x(self, v): pass
    def go(self): pass
    def go(self): pass

async def top(a, b=1) -> int:
    return a

try:
    def compat(): pass
except ImportError:
    def compat(): pass
"#;
        let a = analyze(src, Lang::Python).unwrap();
        let n = names(&a);
        assert!(n.contains(&"Class:A".to_string()));
        assert!(n.contains(&"Method:A.go".to_string()));
        assert!(n.contains(&"Function:top".to_string()));
        let top = a.items.iter().find(|i| i.name == "top").unwrap();
        assert!(top.is_async);
        assert_eq!(top.tail, "(a, b=1) -> int");
        assert_eq!(a.items[0].summary.as_deref(), Some("Doc of A."));
        let d = a.find_duplicates();
        let flagged: Vec<_> = d.redefinitions.iter().map(|g| g.name.clone()).collect();
        assert_eq!(flagged, vec!["A.go".to_string()]);
    }

    #[test]
    fn rust_structure() {
        let src = r#"
/// A point.
#[derive(Debug)]
struct Point { x: i32 }

impl Point {
    pub fn new() -> Self { Point { x: 0 } }
    async fn later(&self) {}
}

impl std::fmt::Display for Point {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) }
}

#[cfg(windows)]
fn plat() {}
#[cfg(not(windows))]
fn plat() {}

fn main() {}
"#;
        let a = analyze(src, Lang::Rust).unwrap();
        let n = names(&a);
        assert!(n.contains(&"Struct:Point".to_string()), "{n:?}");
        assert!(n.contains(&"Method:Point.new".to_string()), "{n:?}");
        assert!(n.contains(&"Method:Point.fmt".to_string()), "{n:?}");
        let p = a.items.iter().find(|i| i.name == "Point").unwrap();
        assert_eq!(p.start, 2, "doc + attribute must be included");
        assert_eq!(p.summary.as_deref(), Some("A point."));
        assert!(a.items.iter().find(|i| i.name == "later").unwrap().is_async);
        assert!(a.find_duplicates().redefinitions.is_empty());
        assert_eq!(a.find("Point::new").len(), 1);
    }

    #[test]
    fn go_structure() {
        let src = "package main\n\n// Server serves.\ntype Server struct{}\n\nfunc (s *Server) Start() {}\n\nfunc main() {}\n";
        let a = analyze(src, Lang::Go).unwrap();
        let m = a.items.iter().find(|i| i.name == "Start").unwrap();
        assert_eq!(m.qual, "Server.Start");
        assert!(m.parent.is_some());
    }

    #[test]
    fn js_structure() {
        let src =
            "export class K { constructor(){} run(){} }\nconst f = (a) => a;\nfunction g(){}\n";
        let a = analyze(src, Lang::JavaScript).unwrap();
        let n = names(&a);
        assert!(n.contains(&"Method:K.run".to_string()), "{n:?}");
        assert!(n.contains(&"Function:f".to_string()), "{n:?}");
        assert!(n.contains(&"Function:g".to_string()), "{n:?}");
    }

    #[test]
    fn broken_code_still_parses() {
        let a = analyze("def ok():\n    pass\n\ndef broken(:\n", Lang::Python).unwrap();
        assert!(a.items.iter().any(|i| i.name == "ok"));
        assert!(!a.error_lines.is_empty());
    }
}
