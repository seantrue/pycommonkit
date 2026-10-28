//! Keys and schemes (contract §3): the key for a segment, and the path that key names.
//!
//! ```text
//! key   {domain}:{version}:{form}:{digest}:{variant}
//! path  {form}/{digest[0:2]}/{digest}.{variant}.npy
//! ```
//!
//! The grammar is a safety boundary: a key becomes a path the reaper may later unlink, so every
//! component is matched in full (no final newline either), and nothing that fails it ever names
//! a path. Building a key, or constructing or registering a scheme, from invalid input is one
//! of the contract's two loud paths (§1.6), so those return [`KeyError`]; the store (step 6)
//! turns a bad key on its read and publish paths into a counted miss.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use regex::Regex;

use crate::npy::Descr;

/// Why a key, scheme or registration was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyError {
    /// A domain, version, form or closed-set variant of a scheme is not a token `[a-z0-9]+`.
    Token { part: &'static str, value: String },
    /// A scheme with no forms.
    NoForms,
    /// A form listed twice.
    DuplicateForm(String),
    /// A closed variant set that is empty.
    NoVariants,
    /// A variant pattern that does not compile, or uses syntax outside contract §3.2.
    Pattern(String),
    /// A dtype declared for a form the scheme does not have.
    DtypeForm(String),
    /// A dtype that is not a descriptor of contract §5.4.
    Dtype(String),
    /// After ASCII folding, the digest is not 32 lower-case hex digits.
    Digest(String),
    /// A form the scheme does not have.
    UnknownForm(String),
    /// A variant that is not a token, or not valid for the scheme.
    UnknownVariant(String),
    /// A key that fails the grammar of contract §3.1.
    Grammar(String),
    /// A grammatical key whose domain or version is not the scheme's.
    OtherScheme(String),
    /// No scheme is registered for the key's domain.
    Unregistered(String),
    /// A second, different scheme for a registered domain.
    Conflict(String),
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::Token { part, value } => write!(f, "{part} must be [a-z0-9]+, got {value:?}"),
            KeyError::NoForms => f.write_str("a scheme needs at least one form"),
            KeyError::DuplicateForm(form) => write!(f, "form {form:?} is listed twice"),
            KeyError::NoVariants => f.write_str("a closed variant set needs at least one variant"),
            KeyError::Pattern(why) => write!(f, "bad variant pattern: {why}"),
            KeyError::DtypeForm(form) => write!(f, "dtype declared for unknown form {form:?}"),
            KeyError::Dtype(d) => write!(f, "not a descriptor: {d:?}"),
            KeyError::Digest(d) => write!(f, "not a 32-digit hex digest: {d:?}"),
            KeyError::UnknownForm(form) => write!(f, "unknown form {form:?}"),
            KeyError::UnknownVariant(v) => write!(f, "unknown variant {v:?}"),
            KeyError::Grammar(k) => write!(f, "not a key: {k:?}"),
            KeyError::OtherScheme(k) => write!(f, "key {k:?} belongs to another scheme"),
            KeyError::Unregistered(k) => write!(f, "no registered scheme for key {k:?}"),
            KeyError::Conflict(d) => write!(f, "two disagreeing schemes for domain {d:?}"),
        }
    }
}

impl std::error::Error for KeyError {}

/// Is `s` a token: one or more of `[a-z0-9]`, in full?
pub fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Is `s` a digest: exactly 32 of `[0-9a-f]`?
pub fn is_digest(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A key that satisfies the grammar of contract §3.1, whatever scheme it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    text: String,
}

impl Key {
    /// Check the grammar alone (contract §3.1). A reader may attach any grammatical key,
    /// registered or not (C4).
    pub fn parse(key: &str) -> Result<Key, KeyError> {
        let mut parts = key.split(':');
        let ok = match (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) {
            (Some(d), Some(v), Some(f), Some(g), Some(x), None) => {
                is_token(d) && is_token(v) && is_token(f) && is_digest(g) && is_token(x)
            }
            _ => false,
        };
        if !ok {
            return Err(KeyError::Grammar(key.chars().take(200).collect()));
        }
        Ok(Key {
            text: key.to_owned(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    fn part(&self, i: usize) -> &str {
        self.text.split(':').nth(i).unwrap_or("")
    }

    pub fn domain(&self) -> &str {
        self.part(0)
    }

    pub fn version(&self) -> &str {
        self.part(1)
    }

    pub fn form(&self) -> &str {
        self.part(2)
    }

    pub fn digest(&self) -> &str {
        self.part(3)
    }

    pub fn variant(&self) -> &str {
        self.part(4)
    }

    /// `{form}/{digest[0:2]}/{digest}.{variant}.npy`, with `/` as the separator (contract §3.6).
    /// Never absolute; it depends only on the key.
    pub fn relpath(&self) -> String {
        let digest = self.digest();
        let shard = digest.get(..2).unwrap_or("");
        format!("{}/{shard}/{digest}.{}.npy", self.form(), self.variant())
    }

    /// [`Key::relpath`] as a relative [`PathBuf`] of three components, joined with the
    /// platform's separator.
    pub fn relpath_buf(&self) -> PathBuf {
        let digest = self.digest();
        let mut p = PathBuf::from(self.form());
        p.push(digest.get(..2).unwrap_or(""));
        p.push(format!("{digest}.{}.npy", self.variant()));
        p
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// The variants a scheme accepts (contract §3.2).
#[derive(Debug, Clone)]
pub enum Variants {
    /// A closed set of tokens.
    Closed(BTreeSet<String>),
    /// A pattern for an open axis such as sample rates; it must match the whole variant.
    Pattern { text: String, re: Regex },
}

impl Variants {
    /// A closed set. Each must be a token, and the set must not be empty.
    pub fn closed<I, S>(variants: I) -> Result<Variants, KeyError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let set: BTreeSet<String> = variants.into_iter().map(Into::into).collect();
        if set.is_empty() {
            return Err(KeyError::NoVariants);
        }
        if let Some(bad) = set.iter().find(|v| !is_token(v)) {
            return Err(KeyError::Token {
                part: "variant",
                value: bad.clone(),
            });
        }
        Ok(Variants::Closed(set))
    }

    /// A pattern. It is compiled as written first, so a pattern that is not a regular
    /// expression on its own (`a)|(b`) can't be rescued by the anchoring, then anchored to
    /// match in full, as Python's `fullmatch`. The `regex` crate refuses backreferences and
    /// look-around, the syntax contract §3.2 excludes.
    pub fn pattern(text: impl Into<String>) -> Result<Variants, KeyError> {
        let text = text.into();
        let build = |p: &str| {
            regex::RegexBuilder::new(p)
                .size_limit(1 << 20)
                .nest_limit(64)
                .build()
                .map_err(|e| KeyError::Pattern(e.to_string()))
        };
        build(&text)?;
        let re = build(&format!(r"\A(?:{text})\z"))?;
        Ok(Variants::Pattern { text, re })
    }

    fn accepts(&self, variant: &str) -> bool {
        is_token(variant)
            && match self {
                Variants::Closed(set) => set.contains(variant),
                Variants::Pattern { re, .. } => re.is_match(variant),
            }
    }
}

impl PartialEq for Variants {
    /// The same closed set, or the same pattern **text** (contract §3.4).
    fn eq(&self, other: &Variants) -> bool {
        match (self, other) {
            (Variants::Closed(a), Variants::Closed(b)) => a == b,
            (Variants::Pattern { text: a, .. }, Variants::Pattern { text: b, .. }) => a == b,
            _ => false,
        }
    }
}

impl Eq for Variants {}

/// One domain's vocabulary (contract §3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyScheme {
    domain: String,
    version: String,
    forms: Vec<String>,
    variants: Variants,
    dtypes: BTreeMap<String, Descr>,
}

impl KeyScheme {
    /// A scheme. `forms` is ordered and its first is the default form; `dtypes` maps some forms
    /// to the one descriptor (`|u1`, `<f4`, or a structured literal) a segment of that form
    /// must have. Invalid input is a loud path (contract §1.6).
    pub fn new<F, S>(
        domain: &str,
        version: &str,
        forms: F,
        variants: Variants,
        dtypes: &[(&str, &str)],
    ) -> Result<KeyScheme, KeyError>
    where
        F: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for (part, value) in [("domain", domain), ("version", version)] {
            if !is_token(value) {
                return Err(KeyError::Token {
                    part,
                    value: value.to_owned(),
                });
            }
        }
        let forms: Vec<String> = forms.into_iter().map(Into::into).collect();
        if forms.is_empty() {
            return Err(KeyError::NoForms);
        }
        for (i, form) in forms.iter().enumerate() {
            if !is_token(form) {
                return Err(KeyError::Token {
                    part: "form",
                    value: form.clone(),
                });
            }
            if forms.get(..i).is_some_and(|before| before.contains(form)) {
                return Err(KeyError::DuplicateForm(form.clone()));
            }
        }
        let mut map = BTreeMap::new();
        for (form, descr) in dtypes {
            if !forms.iter().any(|f| f == form) {
                return Err(KeyError::DtypeForm((*form).to_owned()));
            }
            let d = Descr::parse(descr).map_err(|_| KeyError::Dtype((*descr).to_owned()))?;
            map.insert((*form).to_owned(), d);
        }
        Ok(KeyScheme {
            domain: domain.to_owned(),
            version: version.to_owned(),
            forms,
            variants,
            dtypes: map,
        })
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn forms(&self) -> &[String] {
        &self.forms
    }

    pub fn variants(&self) -> &Variants {
        &self.variants
    }

    /// The first form.
    pub fn default_form(&self) -> &str {
        self.forms.first().map_or("", String::as_str)
    }

    /// The descriptor `form` must have, or `None` when any non-object dtype goes.
    pub fn dtype_for(&self, form: &str) -> Option<&Descr> {
        self.dtypes.get(form)
    }

    pub fn has_form(&self, form: &str) -> bool {
        self.forms.iter().any(|f| f == form)
    }

    /// A token, and in the closed set or matched whole by the pattern (contract §3.2).
    pub fn valid_variant(&self, variant: &str) -> bool {
        self.variants.accepts(variant)
    }

    /// Build a key (contract §3.3). The digest is folded to lower case over ASCII letters only;
    /// forms and variants are never folded. `form: None` is the default form.
    pub fn key(&self, digest: &str, variant: &str, form: Option<&str>) -> Result<Key, KeyError> {
        let digest = digest.to_ascii_lowercase();
        if !is_digest(&digest) {
            return Err(KeyError::Digest(digest.chars().take(64).collect()));
        }
        let form = form.unwrap_or(self.default_form());
        if !self.has_form(form) {
            return Err(KeyError::UnknownForm(form.to_owned()));
        }
        if !self.valid_variant(variant) {
            return Err(KeyError::UnknownVariant(variant.to_owned()));
        }
        Ok(Key {
            text: format!("{}:{}:{form}:{digest}:{variant}", self.domain, self.version),
        })
    }

    /// Does a grammatical key belong to this scheme: its domain and version, one of its forms,
    /// and a valid variant?
    pub fn check(&self, key: &Key) -> Result<(), KeyError> {
        if key.domain() != self.domain || key.version() != self.version {
            return Err(KeyError::OtherScheme(key.text.clone()));
        }
        if !self.has_form(key.form()) {
            return Err(KeyError::UnknownForm(key.form().to_owned()));
        }
        if !self.valid_variant(key.variant()) {
            return Err(KeyError::UnknownVariant(key.variant().to_owned()));
        }
        Ok(())
    }

    /// Parse a key against this scheme (contract §3.3). Never folds: an upper-case digest is
    /// unparseable.
    pub fn parse(&self, key: &str) -> Result<Key, KeyError> {
        let key = Key::parse(key)?;
        self.check(&key)?;
        Ok(key)
    }

    /// The key's relative path, after checking it against this scheme.
    pub fn relpath(&self, key: &str) -> Result<String, KeyError> {
        self.parse(key).map(|k| k.relpath())
    }
}

/// A set of schemes, at most one per domain (contract §3.4). The process-wide one is
/// [`Registry::global`]; separate ones are for tests and tools.
#[derive(Debug, Default)]
pub struct Registry {
    schemes: Mutex<BTreeMap<String, Arc<KeyScheme>>>,
}

impl Registry {
    pub const fn new() -> Registry {
        Registry {
            schemes: Mutex::new(BTreeMap::new()),
        }
    }

    /// The process-wide registry the store dispatches on.
    pub fn global() -> &'static Registry {
        static GLOBAL: Registry = Registry::new();
        &GLOBAL
    }

    /// Register `scheme`. An identical scheme already registered for the domain is returned
    /// as is; a different one is refused (contract §3.4).
    pub fn register(&self, scheme: KeyScheme) -> Result<Arc<KeyScheme>, KeyError> {
        // A thread that panicked while holding the lock can't have left the map half-updated
        // (each update is one insert), so a poisoned lock is safe to recover.
        let mut map = self.schemes.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(prior) = map.get(&scheme.domain) {
            if **prior == scheme {
                return Ok(Arc::clone(prior));
            }
            return Err(KeyError::Conflict(scheme.domain));
        }
        let scheme = Arc::new(scheme);
        map.insert(scheme.domain.clone(), Arc::clone(&scheme));
        Ok(scheme)
    }

    /// The scheme registered for `domain`.
    pub fn get(&self, domain: &str) -> Option<Arc<KeyScheme>> {
        let map = self.schemes.lock().unwrap_or_else(PoisonError::into_inner);
        map.get(domain).cloned()
    }

    /// The registered scheme that owns `key`, by the text before its first `:`.
    pub fn scheme_for(&self, key: &str) -> Result<Arc<KeyScheme>, KeyError> {
        let domain = key.split(':').next().unwrap_or("");
        self.get(domain)
            .ok_or_else(|| KeyError::Unregistered(key.chars().take(200).collect()))
    }

    /// Parse `key` against the scheme registered for its domain.
    pub fn parse(&self, key: &str) -> Result<Key, KeyError> {
        self.scheme_for(key)?.parse(key)
    }

    /// `key`'s relative path, checked against the scheme registered for its domain.
    pub fn relpath(&self, key: &str) -> Result<String, KeyError> {
        self.parse(key).map(|k| k.relpath())
    }
}

/// Register `scheme` in the process-wide registry.
pub fn register(scheme: KeyScheme) -> Result<Arc<KeyScheme>, KeyError> {
    Registry::global().register(scheme)
}

/// The scheme in the process-wide registry that owns `key`.
pub fn scheme_for(key: &str) -> Result<Arc<KeyScheme>, KeyError> {
    Registry::global().scheme_for(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed() -> KeyScheme {
        KeyScheme::new(
            "ut",
            "v1",
            ["pil"],
            Variants::closed(["o0"]).unwrap(),
            &[("pil", "|u1")],
        )
        .unwrap()
    }

    #[test]
    fn scheme_construction_is_checked() {
        let v = || Variants::closed(["o0"]).unwrap();
        let none: [&str; 0] = [];
        assert_eq!(
            KeyScheme::new("Ut", "v1", ["a"], v(), &[]).unwrap_err(),
            KeyError::Token {
                part: "domain",
                value: "Ut".into()
            }
        );
        assert_eq!(
            KeyScheme::new("ut", "v1", none, v(), &[]).unwrap_err(),
            KeyError::NoForms
        );
        assert_eq!(
            KeyScheme::new("ut", "v1", ["a", "a"], v(), &[]).unwrap_err(),
            KeyError::DuplicateForm("a".into())
        );
        assert!(matches!(
            KeyScheme::new("ut", "v1", ["a"], v(), &[("b", "|u1")]),
            Err(KeyError::DtypeForm(_))
        ));
        assert!(matches!(
            KeyScheme::new("ut", "v1", ["a"], v(), &[("a", "|O")]),
            Err(KeyError::Dtype(_))
        ));
        assert_eq!(Variants::closed(none).unwrap_err(), KeyError::NoVariants);
        assert!(Variants::closed(["o-1"]).is_err());
        for bad in ["a)|(b", "(a", r"(a)\1", "(?=a)a", "(?<=a)b"] {
            assert!(Variants::pattern(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn dtype_identity_is_canonical() {
        let s = closed();
        assert_eq!(s.dtype_for("pil"), Some(&Descr::parse("<u1").unwrap()));
        assert_ne!(s.dtype_for("pil"), Some(&Descr::parse("|i1").unwrap()));
    }

    #[test]
    fn separate_registries_are_separate() {
        let a = Registry::new();
        let b = Registry::new();
        a.register(closed()).unwrap();
        assert!(a.get("ut").is_some());
        assert!(b.get("ut").is_none());
        assert!(Arc::ptr_eq(
            &a.register(closed()).unwrap(),
            &a.get("ut").unwrap()
        ));
        let k = a
            .parse("ut:v1:pil:abababababababababababababababab:o0")
            .unwrap();
        assert_eq!(
            k.relpath_buf(),
            PathBuf::from("pil")
                .join("ab")
                .join(format!("{}.o0.npy", "ab".repeat(16)))
        );
        assert!(matches!(
            b.parse(k.as_str()),
            Err(KeyError::Unregistered(_))
        ));
    }

    #[test]
    fn digest_folds_ascii_only() {
        let s = closed();
        // U+212A KELVIN SIGN lower-cases to `k` under Unicode folding; it must not here.
        assert!(
            s.key(&format!("{}\u{212a}", "a".repeat(31)), "o0", None)
                .is_err()
        );
    }
}
