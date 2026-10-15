use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn watch(path: &Path, files: &mut Vec<PathBuf>) {
    // Absolute paths keep a shared target's fingerprint tied to the checkout
    // whose assets were embedded. Track files as well as directory additions.
    println!("cargo:rerun-if-changed={}", path.display());
    if path.is_dir() {
        for entry in fs::read_dir(path)
            .expect("dashboard asset directory")
            .flatten()
        {
            watch(&entry.path(), files);
        }
    } else if !path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with('.'))
    {
        // Dotfiles (.DS_Store and friends) are local noise, never shipped
        // content, so they must not move the version between machines.
        files.push(path.to_path_buf());
    }
}

/// FNV-1a, 64 bit. Dependency-free and stable across Rust releases, which
/// `std::hash` is not. Collision resistance against an adversary is not a
/// requirement: the value only has to change when the shipped bytes do.
fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *hash ^= u64::from(*b);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// The dashboard asset version: a hash of every file under `static/`, keyed by
/// its path relative to `static/` (sorted, `/`-separated) and its bytes.
///
/// app.js and sw.js carry `__AMUX_ASSET_VER__` where a hand-bumped literal used
/// to be, and the server substitutes this value when it serves them (see
/// `amux_dashboard::asset`). Every client-visible byte change therefore moves
/// the version, and the version can never disagree between the two files. The
/// literal used to make every pair of dashboard PRs conflict on the same line.
fn asset_version(root: &Path, files: &[PathBuf]) -> String {
    let mut rel: Vec<(String, &PathBuf)> = files
        .iter()
        .map(|p| {
            let r = p
                .strip_prefix(root)
                .expect("asset under static/")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            (r, p)
        })
        .collect();
    rel.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (name, path) in rel {
        fnv1a(&mut hash, name.as_bytes());
        fnv1a(&mut hash, &[0]);
        let bytes = fs::read(path).expect("readable dashboard asset");
        fnv1a(&mut hash, &(bytes.len() as u64).to_le_bytes());
        fnv1a(&mut hash, &bytes);
    }
    format!("{hash:016x}")[..12].to_string()
}

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_MANIFEST_DIR");
    let root = env::var("CARGO_MANIFEST_DIR").expect("dashboard manifest directory");
    let static_dir = Path::new(&root).join("static");
    let mut files = Vec::new();
    watch(&static_dir, &mut files);
    println!(
        "cargo:rustc-env=AMUX_ASSET_VER={}",
        asset_version(&static_dir, &files)
    );
    println!("cargo:rustc-env=AMUX_DASHBOARD_SOURCE_ROOT={root}");
}
