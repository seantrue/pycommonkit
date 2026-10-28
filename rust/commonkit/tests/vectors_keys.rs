//! Every vector in `conformance/vectors/keys.json` (contract §3).

mod common;

use std::sync::Arc;

use common::{list, load, str_of};
use commonkit::keys::{self, Key, KeyError, KeyScheme, Registry, Variants};
use serde_json::Value;

fn scheme_from(d: &Value, domain: Option<&str>) -> Result<KeyScheme, KeyError> {
    let domain = domain.unwrap_or_else(|| str_of(&d["domain"]));
    let forms: Vec<String> = list(&d["forms"])
        .iter()
        .map(|f| str_of(f).to_owned())
        .collect();
    let variants = match d.get("variant_pattern") {
        Some(Value::String(p)) => Variants::pattern(p.as_str())?,
        _ => Variants::closed(list(&d["variants"]).iter().map(|v| str_of(v).to_owned()))?,
    };
    let dtypes: Vec<(String, String)> = d
        .get("dtypes")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), str_of(v).to_owned()))
                .collect()
        })
        .unwrap_or_default();
    let dtypes: Vec<(&str, &str)> = dtypes
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    KeyScheme::new(domain, str_of(&d["version"]), forms, variants, &dtypes)
}

fn schemes(doc: &Value) -> impl Fn(&str) -> KeyScheme + '_ {
    move |name| scheme_from(&doc["schemes"][name], None).unwrap()
}

#[test]
fn build() {
    let doc = load("keys.json");
    let scheme = schemes(&doc);
    let vectors = list(&doc["build"]);
    assert!(!vectors.is_empty());
    for v in vectors {
        let id = str_of(&v["id"]);
        let s = scheme(str_of(&v["scheme"]));
        let got = s.key(
            str_of(&v["digest"]),
            str_of(&v["variant"]),
            v["form"].as_str(),
        );
        match (&v["expect"]["key"], &v["expect"]["error"], got) {
            (Value::String(want), _, Ok(key)) => assert_eq!(key.as_str(), want, "{id}"),
            (_, Value::String(kind), Err(e)) => {
                let ok = match kind.as_str() {
                    "digest" => matches!(e, KeyError::Digest(_)),
                    "form" => matches!(e, KeyError::UnknownForm(_)),
                    "variant" => matches!(e, KeyError::UnknownVariant(_)),
                    other => panic!("{id}: unknown error kind {other}"),
                };
                assert!(ok, "{id}: wanted a {kind} error, got {e:?}");
            }
            (_, _, got) => panic!("{id}: expected {}, got {got:?}", v["expect"]),
        }
    }
}

#[test]
fn parse() {
    let doc = load("keys.json");
    let scheme = schemes(&doc);
    // The vector file's own expression, with Python's `\Z` spelled as Rust's `\z`.
    let grammar = regex::Regex::new(&str_of(&doc["grammar"]).replace(r"\Z", r"\z")).unwrap();
    for v in list(&doc["parse"]) {
        let id = str_of(&v["id"]);
        let key = str_of(&v["key"]);
        let expect = &v["expect"];
        let grammatical = expect["grammar"].as_bool().unwrap();
        assert_eq!(Key::parse(key).is_ok(), grammatical, "{id}: grammar");
        assert_eq!(
            grammar.is_match(key),
            grammatical,
            "{id}: the vector's own expression"
        );
        let s = scheme(str_of(&v["scheme"]));
        let got = s.parse(key);
        if expect["scheme_ok"].as_bool().unwrap() {
            let k = got.unwrap_or_else(|e| panic!("{id}: {e:?}"));
            let parts: Vec<&str> = list(&expect["parts"]).iter().map(str_of).collect();
            assert_eq!(vec![k.form(), k.digest(), k.variant()], parts, "{id}");
            assert_eq!(k.relpath(), str_of(&expect["relpath"]), "{id}");
            assert_eq!(s.relpath(key).unwrap(), str_of(&expect["relpath"]), "{id}");
            assert!(!std::path::Path::new(&k.relpath()).is_absolute(), "{id}");
        } else {
            assert!(got.is_err(), "{id}: the scheme accepted it");
            assert!(s.relpath(key).is_err(), "{id}");
        }
    }
}

#[test]
fn registry() {
    let doc = load("keys.json");
    for v in list(&doc["registry"]) {
        let id = str_of(&v["id"]);
        let domain = str_of(&v["domain"]);
        let first = || scheme_from(&v["first"], Some(domain)).unwrap();
        let second = || scheme_from(&v["second"], Some(domain)).unwrap();
        // A private registry, and the process-wide one (the vectors' domains are unique).
        let private = Registry::new();
        for (name, register) in [
            (
                "private",
                Box::new(|s| private.register(s)) as Box<dyn Fn(KeyScheme) -> _>,
            ),
            ("global", Box::new(keys::register)),
        ] {
            let a = register(first()).unwrap();
            match (str_of(&v["expect"]), register(second())) {
                ("idempotent", Ok(b)) => assert!(Arc::ptr_eq(&a, &b), "{id} ({name})"),
                ("error", Err(KeyError::Conflict(d))) => assert_eq!(d, domain, "{id} ({name})"),
                (want, got) => panic!("{id} ({name}): expected {want}, got {got:?}"),
            }
        }
        let key = format!(
            "{domain}:{}:{}:{}:x",
            str_of(&v["first"]["version"]),
            str_of(&list(&v["first"]["forms"])[0]),
            "ab".repeat(16)
        );
        assert_eq!(keys::scheme_for(&key).unwrap().domain(), domain, "{id}");
    }
}
