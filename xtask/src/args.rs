//! Minimal argument handling: positionals, `--flag` and `--key value`.

use anyhow::{Result, anyhow};

pub struct Args {
    items: Vec<String>,
}

impl Args {
    pub fn from_env() -> Self {
        Self { items: std::env::args().skip(1).collect() }
    }

    /// Next argument that is not an option (does not start with `--`).
    pub fn next_positional(&mut self) -> Option<String> {
        let i = self.items.iter().position(|a| !a.starts_with("--"))?;
        // Skip values that belong to a preceding `--key`: callers take
        // options before positionals where it matters.
        Some(self.items.remove(i))
    }

    pub fn positional(&mut self, what: &str) -> Result<String> {
        self.next_positional().ok_or_else(|| anyhow!("missing argument: {what}"))
    }

    pub fn flag(&mut self, name: &str) -> bool {
        let key = format!("--{name}");
        if let Some(i) = self.items.iter().position(|a| *a == key) {
            self.items.remove(i);
            true
        } else {
            false
        }
    }

    pub fn value(&mut self, name: &str) -> Option<String> {
        let key = format!("--{name}");
        let prefix = format!("--{name}=");
        if let Some(i) = self.items.iter().position(|a| a.starts_with(&prefix)) {
            let v = self.items.remove(i);
            return Some(v[prefix.len()..].to_string());
        }
        let i = self.items.iter().position(|a| *a == key)?;
        self.items.remove(i);
        (i < self.items.len()).then(|| self.items.remove(i))
    }
}

impl Args {
    /// `--name VALUE` (a value not starting with `--`), `--name=VALUE`, or
    /// a bare `--name` (`Some(None)`); `None` when absent.
    pub fn flag_or_value(&mut self, name: &str) -> Option<Option<String>> {
        let key = format!("--{name}");
        let prefix = format!("--{name}=");
        if let Some(i) = self.items.iter().position(|a| a.starts_with(&prefix)) {
            let v = self.items.remove(i);
            return Some(Some(v[prefix.len()..].to_string()));
        }
        let i = self.items.iter().position(|a| *a == key)?;
        self.items.remove(i);
        if i < self.items.len() && !self.items[i].starts_with("--") {
            Some(Some(self.items.remove(i)))
        } else {
            Some(None)
        }
    }
}

/// Parse `0x…` hex or decimal.
pub fn num(s: &str) -> Result<u32> {
    let s = s.replace('_', "");
    let r = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(h, 16)
    } else {
        s.parse()
    };
    r.map_err(|e| anyhow!("bad number `{s}`: {e}"))
}

impl Args {
    pub fn from_vec(items: Vec<String>) -> Self {
        Self { items }
    }
}
