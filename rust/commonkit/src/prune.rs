//! Prune a folder of files nothing refers to. Python's `commonkit.prune.prune_folder`, checked
//! by the same vectors (`conformance/vectors/prune.json`).
//!
//! Not the store: the store's reaper evicts cache segments by age and size (contract §9). This is
//! for durable files an application keeps in a folder of its own, named for their content (a hand
//! edit, a saved variant), where an old file stays useful while anything can still reach it: an
//! undo history, another document, a list the application keeps. The application says which
//! names are reachable; this removes the rest, carefully:
//!
//! - it looks only at the folder's own entries: it never descends and never follows a symbolic
//!   link, and the folder itself must be a real directory;
//! - a candidate is a regular file (by `lstat`) whose whole name matches the pattern and, when an
//!   owner check is given, that it claims; anything else is foreign and is kept;
//! - a candidate named in `keep` is kept, as is one modified less than `min_age` ago (another
//!   process may hold it in a history this one can't see);
//! - a removed file's companions (its name plus each companion suffix, such as `.json`) go with
//!   it, when they are regular files;
//! - a dry run lists what would go and removes nothing.
//!
//! Nothing here panics or fails for a file it meets: a failed unlink is counted and the walk goes
//! on; a folder or pattern it can't use is refused in the report.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use regex::Regex;

/// What to prune.
#[derive(Debug, Clone, Default)]
pub struct PruneOptions {
    /// A regular expression a candidate's whole name matches.
    pub pattern: String,
    /// Names something can still reach: kept.
    pub keep: BTreeSet<String>,
    /// A candidate modified less than this long ago is kept.
    pub min_age: Duration,
    /// Suffixes whose files (`<name><suffix>`) go with a removed candidate.
    pub companions: Vec<String>,
    /// List what would go; remove nothing.
    pub dry_run: bool,
    /// The time ages are measured from (default: now).
    pub now: Option<SystemTime>,
}

/// What a prune did (or, dry, would do).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PruneReport {
    /// The names removed, companions too, sorted.
    pub removed: Vec<String>,
    /// Candidates kept because something reaches them.
    pub kept: usize,
    /// Candidates kept for being too young.
    pub young: usize,
    /// Entries that aren't candidates (another name, not a regular file, disclaimed).
    pub foreign: usize,
    /// Unlinks that failed.
    pub failed: usize,
    /// Why nothing was looked at, if so.
    pub refused: Option<String>,
}

fn regular(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

/// Remove (or with `dry_run` list) the files in `folder` nothing refers to; `owns`, when given,
/// claims a candidate as the application's (a sidecar naming it, a hash that matches).
pub fn prune_folder(
    folder: &Path,
    options: &PruneOptions,
    owns: Option<&dyn Fn(&Path) -> bool>,
) -> PruneReport {
    let mut report = PruneReport::default();
    match std::fs::symlink_metadata(folder) {
        Ok(m) if m.file_type().is_dir() => {}
        Ok(_) => {
            report.refused = Some(format!(
                "{} is not a directory (a symbolic link is not followed)",
                folder.display()
            ));
            return report;
        }
        Err(e) => {
            report.refused = Some(format!("{}: {e}", folder.display()));
            return report;
        }
    }
    let rx = match Regex::new(&format!("^(?:{})$", options.pattern)) {
        Ok(rx) => rx,
        Err(e) => {
            report.refused = Some(format!("pattern: {e}"));
            return report;
        }
    };
    let mut names: Vec<String> = match std::fs::read_dir(folder) {
        Ok(entries) => entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .collect(),
        Err(e) => {
            report.refused = Some(format!("{}: {e}", folder.display()));
            return report;
        }
    };
    names.sort();
    let present: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let companions: Vec<&str> = options
        .companions
        .iter()
        .map(String::as_str)
        .filter(|c| !c.is_empty())
        .collect();
    let now = options.now.unwrap_or_else(SystemTime::now);
    for name in &names {
        let path = folder.join(name);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        // a companion of some candidate is judged with it, not on its own
        if companions.iter().any(|c| {
            name.strip_suffix(c)
                .is_some_and(|stem| present.contains(stem) && rx.is_match(stem))
        }) {
            continue;
        }
        if !meta.file_type().is_file() || !rx.is_match(name) || owns.is_some_and(|o| !o(&path)) {
            report.foreign += 1;
            continue;
        }
        if options.keep.contains(name) {
            report.kept += 1;
            continue;
        }
        let age = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .unwrap_or(Duration::ZERO);
        if age < options.min_age {
            report.young += 1;
            continue;
        }
        let mut going = vec![name.clone()];
        for c in &companions {
            let n = format!("{name}{c}");
            if present.contains(n.as_str()) && regular(&folder.join(&n)) {
                going.push(n);
            }
        }
        for g in going {
            if !options.dry_run && std::fs::remove_file(folder.join(&g)).is_err() {
                report.failed += 1;
                continue;
            }
            report.removed.push(g);
        }
    }
    report.removed.sort();
    report
}
