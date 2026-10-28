//! The Rust side of the cross-language conformance driver (plan step 8). Test tooling, never
//! published.
//!
//! `ck-conform` drives the `commonkit` crate from the command line and prints JSON;
//! `conformance/ck_conform.py` drives the Python package with the same commands and the same
//! JSON, so a test can run any case against either language and compare. The commands, their
//! arguments and results are documented in that file's docstring; this one follows it.
//!
//! The same functions are used in-process by this crate's tests, which call the Python driver
//! with `uv run python` for the other direction.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use commonkit::npy::{ByteOrder, Descr, Field, Kind, Order};
use commonkit::store::Hooks;
use commonkit::{
    ArrayRef, AttachMode, KeyScheme, Mode, Options, Policy, ReapOptions, ReapReport, Segment,
    Store, Variants,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub type Error = String;

/// Register the conformance scheme, identically to the Python driver's `SCHEME`.
pub fn register_scheme() -> Result<(), Error> {
    static DONE: OnceLock<Result<(), Error>> = OnceLock::new();
    DONE.get_or_init(|| {
        let scheme = KeyScheme::new(
            "ckconf",
            "v1",
            ["raw", "img", "f4"],
            Variants::closed(["x0", "x1"]).map_err(|e| e.to_string())?,
            &[("img", "|u1"), ("f4", "<f4")],
        )
        .map_err(|e| e.to_string())?;
        commonkit::keys::register(scheme)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .clone()
}

fn f64_of(v: &Map<String, Value>, k: &str, default: f64) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(default)
}

fn i64_of(v: &Map<String, Value>, k: &str, default: i64) -> i64 {
    match v.get(k) {
        Some(x) => x
            .as_i64()
            .or_else(|| x.as_f64().map(|f| f as i64))
            .unwrap_or(default),
        None => default,
    }
}

fn str_of<'a>(v: &'a Value, k: &str) -> Result<&'a str, Error> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string {k:?}"))
}

/// Store options from a `store` object, with the drivers' shared defaults (no gate in the way,
/// no reap turn unless asked for, `verify_republish` on as the conformance suite runs it).
pub fn store_from(args: &Value) -> Result<Store, Error> {
    let s = args
        .get("store")
        .and_then(Value::as_object)
        .ok_or("missing \"store\"")?;
    let root = s
        .get("root")
        .and_then(Value::as_str)
        .ok_or("missing store root")?;
    let mode = match s.get("mode").and_then(Value::as_str).unwrap_or("readwrite") {
        "readwrite" => Mode::ReadWrite,
        "read" => Mode::Read,
        "off" => Mode::Off,
        m => return Err(format!("mode {m}")),
    };
    let attach = match s.get("attach").and_then(Value::as_str).unwrap_or("mmap") {
        "mmap" => AttachMode::Mmap,
        "read" => AttachMode::Read,
        a => return Err(format!("attach {a}")),
    };
    let options = Options {
        mode,
        attach,
        max_bytes: i64_of(s, "max_bytes", 1 << 40),
        min_free_bytes: i64_of(s, "min_free_bytes", 0),
        ttl_s: f64_of(s, "ttl_s", 1e9),
        min_array_bytes: u64::try_from(i64_of(s, "min_array_bytes", 0)).unwrap_or(0),
        max_array_bytes: i64_of(s, "max_array_bytes", 1 << 40),
        reap_interval_s: f64_of(s, "reap_interval_s", 1e12),
        fsync: s.get("fsync").and_then(Value::as_bool).unwrap_or(false),
        verify_republish: s
            .get("verify_republish")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    };
    Ok(Store::new(root, options))
}

/// An owned array built from a contract §13.2 spec, with `fill` or `payload_hex`.
pub struct Array {
    pub descr: Descr,
    pub shape: Vec<u64>,
    pub order: Order,
    pub data: Vec<u8>,
}

impl Array {
    pub fn from_spec(spec: &Value) -> Result<Array, Error> {
        let descr = spec_descr(spec)?;
        let shape: Vec<u64> = spec
            .get("shape")
            .and_then(Value::as_array)
            .ok_or("missing shape")?
            .iter()
            .map(|d| d.as_u64().ok_or("bad dimension"))
            .collect::<Result<_, _>>()?;
        let order = match spec.get("order").and_then(Value::as_str).unwrap_or("C") {
            "C" => Order::C,
            "F" => Order::F,
            o => return Err(format!("order {o}")),
        };
        let n: u64 = shape
            .iter()
            .try_fold(1u64, |a, &d| a.checked_mul(d))
            .ok_or("shape overflows")?;
        let data = match spec.get("payload_hex").and_then(Value::as_str) {
            Some(hex) => from_hex(hex)?,
            None => fill(
                &descr,
                n,
                spec.get("fill").and_then(Value::as_str).unwrap_or("ramp"),
            )?,
        };
        Ok(Array {
            descr,
            shape,
            order,
            data,
        })
    }

    pub fn as_ref(&self) -> Result<ArrayRef<'_>, Error> {
        ArrayRef::new(&self.descr, &self.shape, self.order, &self.data).map_err(|e| e.to_string())
    }
}

fn spec_descr(spec: &Value) -> Result<Descr, Error> {
    if let Some(f) = spec.get("fields") {
        let count = f
            .get("count")
            .and_then(Value::as_u64)
            .ok_or("fields.count")?;
        let digits = f
            .get("digits")
            .and_then(Value::as_u64)
            .ok_or("fields.digits")? as usize;
        let each = Descr::parse(str_of(f, "descr")?).map_err(|e| e.to_string())?;
        let fields = (0..count)
            .map(|i| Field::new(format!("f{i:0digits$}"), each.clone(), vec![]))
            .collect();
        return Descr::structured(fields).map_err(|e| e.to_string());
    }
    Descr::parse(str_of(spec, "descr")?).map_err(|e| e.to_string())
}

fn from_hex(hex: &str) -> Result<Vec<u8>, Error> {
    if !hex.len().is_multiple_of(2) {
        return Err("odd hex".into());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            hex.get(i..i + 2)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| "bad hex".to_owned())
        })
        .collect()
}

/// The generator's `_fill` (contract §13.2): deterministic payload bytes for `n_items`
/// elements, valid for the kind; `ramp` and `ramp2` differ in every byte.
pub fn fill(descr: &Descr, n_items: u64, fill: &str) -> Result<Vec<u8>, Error> {
    let nbytes = descr
        .itemsize()
        .and_then(|s| s.checked_mul(n_items))
        .and_then(|n| usize::try_from(n).ok())
        .ok_or("payload too large")?;
    let seed: u64 = match fill {
        "ramp" => 11,
        "ramp2" => 101,
        "zeros" => return Ok(vec![0; nbytes]),
        f => return Err(format!("fill {f}")),
    };
    if nbytes == 0 {
        return Ok(Vec::new());
    }
    let generic = || {
        (0..nbytes as u64)
            .map(|i| ((i * 37 + seed) & 0xff) as u8)
            .collect()
    };
    let Descr::Simple(s) = descr else {
        return Ok(generic());
    };
    let big = s.order() == ByteOrder::Big;
    Ok(match s.kind() {
        Kind::Bool => (0..nbytes).map(|i| (i & 1) as u8).collect(),
        Kind::Bytes => (0..nbytes as u64)
            .map(|i| (97 + (i + seed) % 26) as u8)
            .collect(),
        Kind::Unicode => (0..nbytes as u64 / 4)
            .flat_map(|i| {
                let c = (97 + (i + seed) % 26) as u32;
                if big {
                    c.to_be_bytes()
                } else {
                    c.to_le_bytes()
                }
            })
            .collect(),
        // Little-endian even for `>M8`: the generator computes `arange(n, dtype=">i8") * 1000
        // + seed`, and numpy arithmetic returns native (little-endian) order, so that is what
        // the fixtures hold (found by step 8's cross-language publish of `big_M8_s`).
        Kind::Datetime | Kind::Timedelta => (0..n_items)
            .flat_map(|i| ((i * 1000 + seed) as i64).to_le_bytes())
            .collect(),
        _ => generic(),
    })
}

pub fn sha256(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn stats(store: &Store) -> Value {
    Value::Object(
        store
            .drain_stats()
            .iter()
            .filter(|&(_, v)| v != 0)
            .map(|(k, v)| (k.to_owned(), json!(v)))
            .collect(),
    )
}

/// What an attach found, as the Python driver's `_describe`.
pub fn describe(seg: &Segment) -> Value {
    json!({
        "hit": true,
        "descr": seg.descr_text(),
        "shape": seg.shape(),
        "fortran_order": seg.fortran_order(),
        "payload_sha256": sha256(seg.bytes()),
        "mapped": seg.is_mapped(),
    })
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        a.extend(b);
    }
    a
}

fn cmd_publish(args: &Value) -> Result<Value, Error> {
    register_scheme()?;
    let store = store_from(args)?;
    let array = Array::from_spec(args.get("array").ok_or("missing array")?)?;
    let out = match store.publish(str_of(args, "key")?, array.as_ref()?) {
        Ok(stored) => json!({ "stored": stored }),
        Err(_) => json!({ "collision": true }),
    };
    Ok(merge(out, json!({ "stats": stats(&store) })))
}

fn cmd_remove(args: &Value) -> Result<Value, Error> {
    register_scheme()?;
    let store = store_from(args)?;
    let removed = store.remove(str_of(args, "key")?);
    Ok(json!({ "removed": removed, "stats": stats(&store) }))
}

fn cmd_attach(args: &Value) -> Result<Value, Error> {
    register_scheme()?;
    let store = store_from(args)?;
    let out = match store.attach(str_of(args, "key")?) {
        Some(seg) => describe(&seg),
        None => json!({ "hit": false }),
    };
    Ok(merge(out, json!({ "stats": stats(&store) })))
}

fn cmd_hold(args: &Value) -> Result<Value, Error> {
    register_scheme()?;
    let store = store_from(args)?;
    let key = str_of(args, "key")?;
    let seg = store.attach(key);
    let first = match &seg {
        Some(seg) => describe(seg),
        None => json!({ "hit": false }),
    };
    emit(&merge(first, json!({ "stats": stats(&store) })));
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    Ok(match seg {
        None => json!({ "hit": false }),
        Some(seg) => json!({
            "payload_sha256": sha256(seg.bytes()),
            "path_exists": store.path_for(key).is_some_and(|p| p.exists()),
        }),
    })
}

fn rel(root: &Path, paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .unwrap_or(p)
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

fn report(root: &Path, r: &ReapReport) -> Value {
    let mut out = json!({
        "scanned": r.scanned,
        "reaped": r.reaped,
        "bytes": r.bytes,
        "complete": r.complete,
        "dry_run": r.dry_run,
    });
    if r.dry_run {
        out = merge(
            out,
            json!({
                "unlink": rel(root, &r.unlink),
                "sweep": rel(root, &r.sweep),
                "low_water": r.low_water,
                "need_free": r.need_free,
            }),
        );
    }
    out
}

fn reap_options(args: &Value, default_budget: f64) -> ReapOptions {
    ReapOptions {
        budget: Duration::from_secs_f64(
            args.get("budget_s")
                .and_then(Value::as_f64)
                .unwrap_or(default_budget)
                .clamp(0.0, 1e9),
        ),
        dry_run: args
            .get("dry_run")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        now: args.get("now").and_then(Value::as_f64),
    }
}

fn cmd_reap(args: &Value) -> Result<Value, Error> {
    let mut store = store_from(args)?;
    let mut hooks = Hooks::default();
    if args
        .get("die_in_walk")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        // Exit holding `.reap.lock`, as a crashed reaper: `exit` runs no destructors, so the
        // lock's guard never unlinks it.
        hooks.reap_dir = Some(Arc::new(|_: &Path| std::process::exit(17)));
    }
    if let Some(pause) = args.get("sleep_before_unlink").and_then(Value::as_f64) {
        let paused = Arc::new(AtomicBool::new(false));
        hooks.reap_unlink = Some(Arc::new(move |p: &Path| {
            let is_segment = p.extension().is_some_and(|e| e == "npy");
            if is_segment && !paused.swap(true, Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs_f64(pause.clamp(0.0, 600.0)));
            }
            std::fs::remove_file(p)
        }));
    }
    store = store.with_hooks(hooks);
    let r = store.reap_with(&reap_options(args, 10.0));
    Ok(merge(
        report(store.root(), &r),
        json!({ "stats": stats(&store) }),
    ))
}

fn policy_json(p: &Policy) -> Value {
    json!({
        "max_bytes": p.max_bytes,
        "min_free_bytes": p.min_free_bytes,
        "ttl_s": p.ttl_s,
        "reap_interval_s": p.reap_interval_s,
        "max_array_bytes": p.max_array_bytes,
        "fsync": p.fsync,
        "verify_republish": p.verify_republish,
    })
}

fn tree_policy(args: &Value) -> Result<(PathBuf, Policy), Error> {
    let root = std::path::absolute(str_of(args, "root")?).map_err(|e| e.to_string())?;
    let policy = Policy::for_root(&root, &commonkit::policy::process_env);
    Ok((root, policy))
}

fn cmd_plan(args: &Value) -> Result<Value, Error> {
    let (root, policy) = tree_policy(args)?;
    let options = policy.options(Mode::Read, AttachMode::Mmap, 0);
    let r = commonkit::reap_plan(&root, &options, &reap_options(args, 60.0));
    Ok(merge(
        report(&root, &r),
        json!({ "policy": policy_json(&policy) }),
    ))
}

fn cmd_policy(args: &Value) -> Result<Value, Error> {
    Ok(policy_json(&tree_policy(args)?.1))
}

fn cmd_usage(args: &Value) -> Result<Value, Error> {
    let store = store_from(args)?;
    let usage = store
        .usage()
        .map(|u| json!({ "bytes": u.bytes, "files": u.files }));
    Ok(json!({ "usage": usage, "stats": stats(&store) }))
}

pub const COMMANDS: [&str; 7] = [
    "publish", "attach", "hold", "reap", "plan", "usage", "policy",
];

/// Run one command. `start_at` (Unix seconds) waits until then, to start with other processes.
pub fn run(cmd: &str, args: &Value) -> Result<Value, Error> {
    if let Some(at) = args.get("start_at").and_then(Value::as_f64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        if at > now {
            std::thread::sleep(Duration::from_secs_f64((at - now).min(600.0)));
        }
    }
    match cmd {
        "publish" => cmd_publish(args),
        "remove" => cmd_remove(args),
        "attach" => cmd_attach(args),
        "hold" => cmd_hold(args),
        "reap" => cmd_reap(args),
        "plan" => cmd_plan(args),
        "usage" => cmd_usage(args),
        "policy" => cmd_policy(args),
        other => Err(format!("unknown command {other}")),
    }
}

/// One line of JSON on stdout, flushed; keys sorted, as the Python driver's `sort_keys`.
pub fn emit(v: &Value) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}
