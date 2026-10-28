//! The one store per host that every commonkit user shares, and its tree-wide policy
//! (contract §10).
//!
//! Tree-wide policy (byte budget, TTL, free-space floor, reap interval, largest array, fsync,
//! republish verification) applies to every segment in the tree whichever process reaps, so it
//! comes only from the tree's `.policy.json`, the environment (`COMMONKIT_STORE_*`) and the
//! platform defaults, never from a caller. The file wins over the environment (C7): the
//! environment is per process and the tree is not (on thresher an app launched from the Finder
//! ran without `~/.bashrc`'s policy and reaped a 7 GB tree to 2 GB). A caller chooses only its
//! mode, its attach mode and its smallest array worth storing. The root comes from
//! `COMMONKIT_STORE_ROOT` or the platform default, never from the file, which is found by it.
//!
//! The reference is Python's `commonkit.shared`. Nothing here panics or fails: a malformed value
//! is skipped and the next source applies.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::json::{self, Json};
use crate::segment::AttachMode;
use crate::store::{Mode, Options, Store, absolute};

/// The environment variables' prefix.
pub const ENV_PREFIX: &str = "COMMONKIT_STORE_";

/// The tree's policy file, directly under the root (contract §10.5).
pub const POLICY_FILE: &str = ".policy.json";

/// The most of the policy file that is read; a larger one is treated as absent.
pub const POLICY_MAX_BYTES: u64 = 65536;

/// The default smallest array a shared store publishes (contract §10.1).
pub const DEFAULT_MIN_ARRAY_BYTES: u64 = 256 * 1024;

const GIB: f64 = 1_073_741_824.0;

/// The effective tree-wide settings (contract §10.1). Byte counts are already derived:
/// `max_bytes = trunc(MAX_GB × 2^30)` and so on.
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    pub max_bytes: i64,
    pub min_free_bytes: i64,
    pub ttl_s: f64,
    pub reap_interval_s: f64,
    pub max_array_bytes: i64,
    pub fsync: bool,
    pub verify_republish: bool,
}

#[derive(Clone, Copy)]
enum Setting {
    MaxGb,
    MinFreeGb,
    TtlS,
    ReapS,
    MaxArrayBytes,
    Fsync,
    VerifyRepublish,
}

impl Setting {
    fn name(self) -> &'static str {
        match self {
            Setting::MaxGb => "MAX_GB",
            Setting::MinFreeGb => "MIN_FREE_GB",
            Setting::TtlS => "TTL_S",
            Setting::ReapS => "REAP_S",
            Setting::MaxArrayBytes => "MAX_ARRAY_BYTES",
            Setting::Fsync => "FSYNC",
            Setting::VerifyRepublish => "VERIFY_REPUBLISH",
        }
    }

    /// The factor that makes the setting a byte count, for those that are one.
    fn byte_scale(self) -> Option<f64> {
        match self {
            Setting::MaxGb | Setting::MinFreeGb => Some(GIB),
            Setting::MaxArrayBytes => Some(1.0),
            _ => None,
        }
    }
}

impl Policy {
    /// The platform defaults (contract §10.1). macOS: a 2 GB budget, because Time Machine's
    /// local snapshots keep reaped segments' blocks allocated. Every other platform: 8 GB.
    pub fn platform_default() -> Policy {
        Policy {
            max_bytes: if cfg!(target_os = "macos") {
                2 << 30
            } else {
                8 << 30
            },
            min_free_bytes: 8 << 30,
            ttl_s: 900.0,
            reap_interval_s: 30.0,
            max_array_bytes: 128 << 20,
            fsync: false,
            verify_republish: false,
        }
    }

    /// The effective policy (contract §10.6): for each setting, the policy file's valid value,
    /// else the environment's (`env` looks up a full variable name), else the default.
    pub fn resolve(file: &PolicyFile, env: &dyn Fn(&str) -> Option<OsString>) -> Policy {
        let d = Policy::platform_default();
        let bytes = |s: Setting, default: i64| {
            let scale = s.byte_scale().unwrap_or(1.0);
            file.number(s)
                .and_then(|v| derived(v, scale))
                .or_else(|| env_number(env, s).and_then(|v| derived(v, scale)))
                .unwrap_or(default)
        };
        let float = |s: Setting, default: f64| {
            file.number(s)
                .or_else(|| env_number(env, s))
                .unwrap_or(default)
        };
        let flag = |s: Setting, default: bool| {
            file.flag(s).or_else(|| env_flag(env, s)).unwrap_or(default)
        };
        Policy {
            max_bytes: bytes(Setting::MaxGb, d.max_bytes),
            min_free_bytes: bytes(Setting::MinFreeGb, d.min_free_bytes),
            ttl_s: float(Setting::TtlS, d.ttl_s),
            reap_interval_s: float(Setting::ReapS, d.reap_interval_s),
            max_array_bytes: bytes(Setting::MaxArrayBytes, d.max_array_bytes),
            fsync: flag(Setting::Fsync, d.fsync),
            verify_republish: flag(Setting::VerifyRepublish, d.verify_republish),
        }
    }

    /// The policy for the tree at `root`: its `.policy.json`, read now, over `env`.
    pub fn for_root(root: &Path, env: &dyn Fn(&str) -> Option<OsString>) -> Policy {
        Policy::resolve(&read_policy(root), env)
    }

    /// Store options with this policy and the caller's own choices.
    pub fn options(&self, mode: Mode, attach: AttachMode, min_array_bytes: u64) -> Options {
        Options {
            mode,
            attach,
            max_bytes: self.max_bytes,
            min_free_bytes: self.min_free_bytes,
            ttl_s: self.ttl_s,
            min_array_bytes,
            max_array_bytes: self.max_array_bytes,
            reap_interval_s: self.reap_interval_s,
            fsync: self.fsync,
            verify_republish: self.verify_republish,
        }
    }
}

/// The derived byte count `trunc(value × scale)`, or `None` outside `[−2^63, 2^63 − 1]`
/// (contract §10.2): `1e300` GiB is malformed, not a panic.
fn derived(value: f64, scale: f64) -> Option<i64> {
    const TWO_63: f64 = 9_223_372_036_854_775_808.0;
    let n = (value * scale).trunc();
    // Both bounds are exact doubles; a NaN fails both comparisons.
    (-TWO_63..TWO_63).contains(&n).then_some(n as i64)
}

/// A number under the contract's grammar (§10.2): after trimming ASCII whitespace,
/// `[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?` in full, and finite as a double. No
/// `inf`, `nan`, `_`, hex or non-ASCII digits, which Python's `float()` and Rust's `parse` would
/// otherwise disagree on.
pub fn parse_number(raw: &str) -> Option<f64> {
    let text = raw.trim_matches([' ', '\t', '\n', '\u{b}', '\u{c}', '\r']);
    let b = text.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize| {
        let start = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i - start
    };
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let int = digits(&mut i);
    let frac = if b.get(i) == Some(&b'.') {
        i += 1;
        digits(&mut i)
    } else {
        0
    };
    if int == 0 && frac == 0 {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return None;
        }
    }
    if i != b.len() {
        return None;
    }
    // The grammar is a subset of what `f64::from_str` accepts, and it rounds to nearest.
    let v: f64 = text.parse().ok()?;
    v.is_finite().then_some(v)
}

/// A flag (contract §10.3): on for exactly `1`, `true`, `yes` or `on`, ASCII case-insensitive,
/// nothing trimmed (`" 1"` is off).
pub fn parse_flag(raw: &str) -> bool {
    ["1", "true", "yes", "on"]
        .iter()
        .any(|on| raw.eq_ignore_ascii_case(on))
}

fn env_value(env: &dyn Fn(&str) -> Option<OsString>, s: Setting) -> Option<OsString> {
    env(&format!("{ENV_PREFIX}{}", s.name())).filter(|v| !v.is_empty())
}

fn env_number(env: &dyn Fn(&str) -> Option<OsString>, s: Setting) -> Option<f64> {
    parse_number(env_value(env, s)?.to_str()?)
}

/// A set, non-empty flag is on or off, never skipped (as Python's `_from_env`); a value that
/// isn't Unicode is off.
fn env_flag(env: &dyn Fn(&str) -> Option<OsString>, s: Setting) -> Option<bool> {
    Some(env_value(env, s)?.to_str().is_some_and(parse_flag))
}

/// The process environment, as [`Policy::resolve`] takes it.
pub fn process_env(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// A tree's `.policy.json`, as read: valid, or absent (contract §10.5).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PolicyFile {
    entries: BTreeMap<String, Json>,
}

impl PolicyFile {
    /// Whether the file was valid and named anything (an empty object reads the same as absent).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A number setting from a JSON number or a string valid under §10.2. Anything else is
    /// left out.
    fn number(&self, s: Setting) -> Option<f64> {
        match self.entries.get(s.name())? {
            Json::Number { text, .. } => text.parse::<f64>().ok().filter(|v| v.is_finite()),
            Json::Str(raw) => parse_number(raw),
            _ => None,
        }
    }

    /// A flag from JSON `true`/`false` or a string read under §10.3. Anything else is left out.
    fn flag(&self, s: Setting) -> Option<bool> {
        match self.entries.get(s.name())? {
            Json::Bool(b) => Some(*b),
            Json::Str(raw) => Some(parse_flag(raw)),
            _ => None,
        }
    }
}

/// Read `{root}/.policy.json`. Absent, entirely, unless it is a regular file of at most 64 KiB
/// holding UTF-8 JSON whose top level is an object with no repeated key (contract §10.5).
/// `NaN` and `Infinity` are not JSON. Never fails, and never blocks on a FIFO.
pub fn read_policy(root: &Path) -> PolicyFile {
    read_policy_inner(&root.join(POLICY_FILE)).unwrap_or_default()
}

fn read_policy_inner(path: &Path) -> Option<PolicyFile> {
    // Checked before opening: opening a FIFO someone named .policy.json would block forever.
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > POLICY_MAX_BYTES {
        return None;
    }
    let mut data = Vec::new();
    File::open(path)
        .ok()?
        .take(POLICY_MAX_BYTES + 1)
        .read_to_end(&mut data)
        .ok()?;
    if u64::try_from(data.len()).ok()? > POLICY_MAX_BYTES {
        return None;
    }
    let Json::Object(pairs) = json::parse(std::str::from_utf8(&data).ok()?)? else {
        return None;
    };
    let count = pairs.len();
    let entries: BTreeMap<String, Json> = pairs.into_iter().collect();
    // A repeated key: JSON parsers resolve it differently, so the file means nothing.
    (entries.len() == count).then_some(PolicyFile { entries })
}

// ------------------------------------------------------------------------------- roots

/// The platform's default root (contract §10.1), or `None` when it can't be formed.
///
/// - macOS: `~/Library/Caches/commonkit/store`, on the internal SSD (no `/dev/shm`; the SSD
///   beat a RAM disk on first attach).
/// - Windows: `%LOCALAPPDATA%\commonkit\store`. Not yet contract (§12); it is the viewer app's
///   existing practice.
/// - Every other platform: `/dev/shm/commonkit`.
pub fn default_root() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(std::env::home_dir()?.join("Library/Caches/commonkit/store"))
    } else if cfg!(windows) {
        let base = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?;
        Some(PathBuf::from(base).join("commonkit").join("store"))
    } else {
        Some(PathBuf::from("/dev/shm/commonkit"))
    }
}

/// The root (contract §10.4): `COMMONKIT_STORE_ROOT` when set and non-empty, with a leading
/// `~` (alone, or before a separator) expanded to the home directory, else the platform default.
/// A relative root is resolved against the working directory when the store is made.
pub fn resolve_root(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    match env(&format!("{ENV_PREFIX}ROOT")).filter(|v| !v.is_empty()) {
        Some(raw) => Some(expand_tilde(Path::new(&raw))),
        None => default_root(),
    }
}

fn expand_tilde(path: &Path) -> PathBuf {
    let mut comps = path.components();
    if comps.next() == Some(Component::Normal("~".as_ref()))
        && let Some(home) = std::env::home_dir()
    {
        return home.join(comps.as_path());
    }
    path.to_path_buf()
}

/// The host's shared store: the root from [`resolve_root`], the policy from its `.policy.json`
/// (read once, now) and the environment. Never fails: an unusable root gives a disabled store.
pub fn shared_store(mode: Mode, attach: AttachMode, min_array_bytes: u64) -> Store {
    match resolve_root(&process_env) {
        Some(root) => shared_store_at(root, mode, attach, min_array_bytes),
        None => Store::new("", Options::default()),
    }
}

/// [`shared_store`] at an explicit root, for tests and tools that must stay out of the shared
/// tree. The policy still comes from that root's file and the environment.
pub fn shared_store_at(
    root: impl AsRef<Path>,
    mode: Mode,
    attach: AttachMode,
    min_array_bytes: u64,
) -> Store {
    let root = root.as_ref();
    let file_root = absolute(root).unwrap_or_else(|| root.to_path_buf());
    let policy = Policy::for_root(&file_root, &process_env);
    Store::new(root, policy.options(mode, attach, min_array_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_grammar() {
        for (raw, want) in [
            ("1800", Some(1800.0)),
            (" 7\t\n", Some(7.0)),
            ("5.", Some(5.0)),
            (".5", Some(0.5)),
            ("-5", Some(-5.0)),
            ("1E+3", Some(1000.0)),
            ("", None),
            (".", None),
            ("-", None),
            ("1e", None),
            ("inf", None),
            ("nan", None),
            ("1_000", None),
            ("0x10", None),
            ("1e999", None),
            ("\u{a0}5", None),
            ("5 5", None),
        ] {
            assert_eq!(parse_number(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn flags() {
        assert!(parse_flag("TRUE") && parse_flag("oN") && parse_flag("1"));
        assert!(!parse_flag(" 1") && !parse_flag("y") && !parse_flag("") && !parse_flag("ON\u{0}"));
    }

    #[test]
    fn derived_bounds() {
        assert_eq!(derived(1e-9, GIB), Some(1));
        assert_eq!(derived(-1.5, 1.0), Some(-1));
        assert_eq!(derived(1e300, GIB), None);
        assert_eq!(derived(9_223_372_036_854_775_808.0, 1.0), None);
        assert_eq!(derived(-9_223_372_036_854_775_808.0, 1.0), Some(i64::MIN));
    }
}
