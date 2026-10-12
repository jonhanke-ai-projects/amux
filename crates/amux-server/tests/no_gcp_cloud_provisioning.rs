//! amux cloud runs on Hetzner. The GCP VM `amux-dev` is deleted for good
//! (Ethan, 2026-10-12), and nothing in the repo may create it again.
//!
//! The paths that did were a Terraform config (`cloud/main.tf`), its wrapper
//! (`cloud/deploy.sh`) and a gateway deploy that reached the VM by name through
//! `gcloud compute ssh`. All were removed or repointed at `$AMUX_CLOUD_HOST`.
//! This guard walks the deploy surface and fails on anything that provisions or
//! addresses a GCP instance, naming the file and line.

use std::path::{Path, PathBuf};

/// Substrings that provision a GCP instance or reach one by name.
const BANNED: &[&str] = &[
    "google_compute_",
    "gcloud compute",
    "hashicorp/google",
    "terraform apply",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn hits(text: &str) -> Vec<(usize, &'static str)> {
    let mut v = Vec::new();
    for (n, line) in text.lines().enumerate() {
        for b in BANNED {
            if line.contains(*b) {
                v.push((n + 1, *b));
            }
        }
    }
    v
}

#[test]
fn nothing_in_the_deploy_surface_creates_a_gcp_instance() {
    let root = root();
    let mut files = vec![root.join("Makefile"), root.join("install.sh")];
    walk(&root.join("cloud"), &mut files);
    walk(&root.join(".github/workflows"), &mut files);
    let mut scanned = 0usize;
    let mut bad = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().to_string();
        // Incident write-ups and the README are history; they deploy nothing.
        if rel.starts_with("cloud/incidents/") || rel.ends_with(".md") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        scanned += 1;
        for (line, what) in hits(&text) {
            bad.push(format!("{rel}:{line}: `{what}`"));
        }
        assert!(!rel.ends_with(".tf"), "{rel}: Terraform config is back under the deploy surface");
    }
    assert!(scanned > 20, "the guard scanned only {scanned} files; it is not looking where it should");
    assert!(
        bad.is_empty(),
        "amux cloud is on Hetzner and the GCP VM is deleted; these lines provision or address a GCP instance:\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_guard_recognises_the_removed_paths() {
    assert_eq!(hits("resource \"google_compute_instance\" \"amux\" {").len(), 1);
    assert_eq!(hits("gcloud compute ssh \"${IAP_USER}@amux-dev\" --tunnel-through-iap").len(), 1);
    assert!(hits("ssh root@$AMUX_CLOUD_HOST systemctl restart amux-gateway").is_empty());
}
