//! Sets `JED_VERSION` from git: the tag on `HEAD` if there is one, else the branch and short
//! commit (`main (8d91187)`), else the commit alone (detached). Without git (a source tarball)
//! it falls back to `Cargo.toml`'s version.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

fn main() {
    // Rebuild when HEAD moves (checkout, commit) or tags change.
    if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        if let Some(head) = git(&["symbolic-ref", "-q", "HEAD"]) {
            let common = git(&["rev-parse", "--git-common-dir"]).unwrap_or(dir);
            if Path::new(&common).join(&head).exists() {
                println!("cargo:rerun-if-changed={common}/{head}");
            }
            println!("cargo:rerun-if-changed={common}/packed-refs");
            println!("cargo:rerun-if-changed={common}/refs/tags");
        }
    }
    let commit = git(&["rev-parse", "--short", "HEAD"]);
    let version = git(&["describe", "--tags", "--exact-match", "HEAD"])
        .or_else(|| {
            let branch = git(&["symbolic-ref", "-q", "--short", "HEAD"])?;
            Some(match &commit {
                Some(c) => format!("{branch} ({c})"),
                None => branch,
            })
        })
        .or(commit)
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=JED_VERSION={version}");
}
