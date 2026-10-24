//! Theme lookup with forgiving names ("solarized-dark" == "Solarized (dark)").

use anyhow::{bail, Result};
use syntect::highlighting::Theme;
use two_face::theme::{extra, LazyThemeSet};

pub struct Themes {
    set: LazyThemeSet,
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

impl Themes {
    pub fn load() -> Self {
        Self {
            set: LazyThemeSet::from(extra()),
        }
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.set.theme_names().map(str::to_owned).collect();
        v.sort_by_key(|n| n.to_lowercase());
        v
    }

    pub fn is_dark(theme: &Theme) -> bool {
        match theme.settings.background {
            Some(c) => (0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32) < 128.0,
            None => true,
        }
    }

    pub fn get(&self, name: &str) -> Option<&Theme> {
        self.set.get(name)
    }

    /// Returns the canonical name and the theme.
    pub fn resolve(&self, query: &str) -> Result<(String, &Theme)> {
        let names = self.names();
        let nq = match norm(query).as_str() {
            "githublight" => "github".to_string(),
            other => other.to_string(),
        };
        let found = names
            .iter()
            .find(|n| norm(n) == nq)
            .or_else(|| names.iter().find(|n| norm(n).starts_with(&nq)))
            .or_else(|| names.iter().find(|n| norm(n).contains(&nq)));
        match found.and_then(|n| self.set.get(n).map(|t| (n.clone(), t))) {
            Some(r) => Ok(r),
            None => bail!("theme '{query}' not found (use -l to list the available themes)"),
        }
    }
}
