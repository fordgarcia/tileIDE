//! Reads the option table passed to a `tileide.*` call, with typed getters.
//! Keys that are never read become "unknown key" warnings instead of errors.

use std::collections::BTreeMap;

use mlua::{Result, Table, Value};

pub struct Fields {
    section: String,
    location: String,
    values: BTreeMap<String, Value>,
}

impl Fields {
    pub fn new(section: impl Into<String>, location: &str, table: Table) -> Result<Self> {
        let section = section.into();
        let mut values = BTreeMap::new();
        for pair in table.pairs::<Value, Value>() {
            let (key, value) = pair?;
            let Value::String(key) = key else {
                return Err(error(location, &format!("{section}: option names must be strings")));
            };
            values.insert(key.to_str()?.to_string(), value);
        }
        Ok(Fields { section, location: location.to_string(), values })
    }

    fn fail(&self, key: &str, message: &str) -> mlua::Error {
        error(&self.location, &format!("{}.{key}: {message}", self.section))
    }

    pub fn string(&mut self, key: &str) -> Result<Option<String>> {
        match self.values.remove(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.to_str()?.to_string())),
            Some(other) => Err(self.fail(key, &format!("expected string, got {}", other.type_name()))),
        }
    }

    pub fn choice(&mut self, key: &str, allowed: &[&str]) -> Result<Option<String>> {
        let Some(value) = self.string(key)? else { return Ok(None) };
        if allowed.contains(&value.as_str()) {
            Ok(Some(value))
        } else {
            Err(self.fail(key, &format!("\"{value}\" is not one of: {}", allowed.join(", "))))
        }
    }

    pub fn boolean(&mut self, key: &str) -> Result<Option<bool>> {
        match self.values.remove(key) {
            None => Ok(None),
            Some(Value::Boolean(b)) => Ok(Some(b)),
            Some(other) => Err(self.fail(key, &format!("expected boolean, got {}", other.type_name()))),
        }
    }

    pub fn number(&mut self, key: &str, min: f64, max: f64) -> Result<Option<f64>> {
        let n = match self.values.remove(key) {
            None => return Ok(None),
            Some(Value::Integer(i)) => i as f64,
            Some(Value::Number(n)) => n,
            Some(other) => return Err(self.fail(key, &format!("expected number, got {}", other.type_name()))),
        };
        if (min..=max).contains(&n) {
            Ok(Some(n))
        } else {
            Err(self.fail(key, &format!("{n} is outside {min}..{max}")))
        }
    }

    pub fn uint(&mut self, key: &str, max: u32) -> Result<Option<u32>> {
        let value = self.values.remove(key);
        // Luau and LuaJIT have no integer subtype, so whole floats count as integers.
        let n = match value {
            None => return Ok(None),
            Some(Value::Integer(i)) => i as f64,
            Some(Value::Number(n)) if n.fract() == 0.0 => n,
            Some(other) => {
                let got = match other {
                    Value::Number(_) => "a fractional number",
                    _ => other.type_name(),
                };
                return Err(self.fail(key, &format!("expected integer, got {got}")));
            }
        };
        if (0.0..=max as f64).contains(&n) {
            Ok(Some(n as u32))
        } else {
            Err(self.fail(key, &format!("{n} is outside 0..{max}")))
        }
    }

    /// Call after reading every known key; the rest are reported, not fatal.
    pub fn finish(self, warnings: &mut Vec<String>) {
        for key in self.values.keys() {
            warnings.push(format!("{}: warning: {}: unknown key \"{key}\" ignored", self.location, self.section));
        }
    }
}

pub fn error(location: &str, message: &str) -> mlua::Error {
    mlua::Error::runtime(format!("{location}: {message}"))
}
