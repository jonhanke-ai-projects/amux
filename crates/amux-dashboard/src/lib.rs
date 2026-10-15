//! Embeds the dashboard SPA static files into the binary at compile time.
//!
//! The SPA lives in `static/` (`index.html`, `app.js`, `app.css`, `sw.js`),
//! extracted from the deleted Python server's inline copy (792ce1f has the
//! history).
//!
//! # Versioning
//!
//! The client version is DERIVED, never hand-bumped. `build.rs` hashes every
//! file under `static/` into [`ASSET_VER`]. `app.js` declares
//! `const APP_VER = '__AMUX_ASSET_VER__';` and `sw.js` declares
//! `const CACHE = 'amux-v__AMUX_ASSET_VER__';`, and [`asset`] substitutes the
//! placeholder when a file is read for serving. So the service-worker cache
//! name moves on every client-visible change, the SSE ping's `v` (api/sse.rs)
//! equals what every freshly loaded page holds, and the two files cannot drift.
//!
//! Serve and inspect dashboard bytes through [`asset`], not
//! `DashboardAssets::get`: the raw embed still carries the placeholder.

use std::borrow::Cow;

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "static/"]
pub struct DashboardAssets;

/// Content hash of `static/`, computed by `build.rs`.
pub const ASSET_VER: &str = env!("AMUX_ASSET_VER");

/// The token in `app.js` / `sw.js` that [`asset`] replaces with [`ASSET_VER`].
pub const ASSET_VER_PLACEHOLDER: &str = "__AMUX_ASSET_VER__";

/// Text assets that may carry the placeholder. Binary files (fonts, wasm,
/// images) are passed through untouched without a scan.
fn may_carry_version(path: &str) -> bool {
    matches!(
        path.rsplit('.').next(),
        Some("js" | "mjs" | "html" | "css" | "json")
    )
}

/// Substitute [`ASSET_VER_PLACEHOLDER`] in `bytes`. Borrowed when absent.
pub fn stamp_version<'a>(bytes: Cow<'a, [u8]>) -> Cow<'a, [u8]> {
    let needle = ASSET_VER_PLACEHOLDER.as_bytes();
    if !bytes.windows(needle.len()).any(|w| w == needle) {
        return bytes;
    }
    let text = String::from_utf8_lossy(&bytes).replace(ASSET_VER_PLACEHOLDER, ASSET_VER);
    Cow::Owned(text.into_bytes())
}

/// The bytes a client receives for `path`: the embedded file with the asset
/// version stamped in. `None` when the file is not embedded.
///
/// The scan and substitution run once per path per process. Stamped copies
/// are leaked into a `'static` cache, which is bounded by the handful of files
/// that carry the placeholder (app.js and sw.js today).
pub fn asset(path: &str) -> Option<Cow<'static, [u8]>> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    /// path -> stamped bytes, or `None` when the file has no placeholder.
    type Stamped = Mutex<HashMap<String, Option<&'static [u8]>>>;
    static STAMPED: OnceLock<Stamped> = OnceLock::new();

    let file = DashboardAssets::get(path)?;
    if !may_carry_version(path) {
        return Some(file.data);
    }
    let cache = STAMPED.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let entry = cache.entry(path.to_string()).or_insert_with(|| {
        match stamp_version(Cow::Borrowed(&file.data[..])) {
            Cow::Borrowed(_) => None,
            Cow::Owned(stamped) => Some(&*Box::leak(stamped.into_boxed_slice())),
        }
    });
    Some(match entry {
        Some(stamped) => Cow::Borrowed(*stamped),
        None => file.data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_a_hash_and_reaches_both_versioned_files() {
        assert_eq!(ASSET_VER.len(), 12);
        assert!(ASSET_VER.chars().all(|c| c.is_ascii_hexdigit()));
        let app = String::from_utf8(asset("app.js").unwrap().into_owned()).unwrap();
        let sw = String::from_utf8(asset("sw.js").unwrap().into_owned()).unwrap();
        assert!(!app.contains(ASSET_VER_PLACEHOLDER));
        assert!(!sw.contains(ASSET_VER_PLACEHOLDER));
        assert!(app.contains(&format!("const APP_VER = '{ASSET_VER}';")));
        assert!(sw.contains(&format!("const CACHE = 'amux-v{ASSET_VER}';")));
    }

    #[test]
    fn stamping_leaves_unversioned_bytes_borrowed() {
        let raw: Cow<'static, [u8]> = Cow::Borrowed(b"no token here");
        assert!(matches!(stamp_version(raw), Cow::Borrowed(_)));
        let owned = stamp_version(Cow::Borrowed(b"v=__AMUX_ASSET_VER__;"));
        assert_eq!(&*owned, format!("v={ASSET_VER};").as_bytes());
    }
}
