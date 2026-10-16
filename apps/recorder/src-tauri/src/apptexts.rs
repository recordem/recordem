// The installed app's own translations, for profile texts with an `app_key`: every <lang>.lproj/Localizable.strings
// in the app bundle, so a profile names Zoom's "LN_Accessibility_Video_Render_194601" and reads "Video render",
// "Rendu vidéo", "ビデオ レンダリング"… in whatever language Zoom runs, with no list kept by hand. Read once per app
// version (plutil, which reads both the text and binary .strings formats).
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

type Table = HashMap<String, Vec<String>>;
static CACHE: LazyLock<Mutex<HashMap<(String, Option<String>), std::sync::Arc<Table>>>> = LazyLock::new(Default::default);

/// key → its text in every language the app has (placeholders like "%@" cut off). Empty for an app without tables.
pub fn table(bundle_id: &str, version: Option<&str>, keys: &[&str]) -> std::sync::Arc<Table> {
    let k = (bundle_id.to_string(), version.map(String::from));
    if let Some(t) = CACHE.lock().unwrap().get(&k) {
        return t.clone();
    }
    let t = std::sync::Arc::new(crate::icons::app_path(bundle_id).map(|app| read(&app, keys)).unwrap_or_default());
    CACHE.lock().unwrap().insert(k, t.clone());
    t
}

fn read(app: &Path, keys: &[&str]) -> Table {
    let mut out: Table = HashMap::new();
    let Ok(dirs) = std::fs::read_dir(app.join("Contents/Resources")) else { return out };
    let files: Vec<PathBuf> = dirs
        .flatten()
        .map(|d| d.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lproj"))
        .map(|p| p.join("Localizable.strings"))
        .filter(|p| p.exists())
        .collect();
    for f in files {
        let Ok(o) = std::process::Command::new("/usr/bin/plutil").args(["-convert", "json", "-o", "-"]).arg(&f).output() else { continue };
        let Ok(map) = serde_json::from_slice::<HashMap<String, serde_json::Value>>(&o.stdout) else { continue };
        for key in keys {
            if let Some(v) = map.get(*key).and_then(|v| v.as_str()) {
                let v = v.split('%').next().unwrap_or("").trim();
                let list = out.entry(key.to_string()).or_default();
                if !v.is_empty() && !list.iter().any(|x| x == v) {
                    list.push(v.to_string());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// `RECORDEM_APP_BUNDLE=/path/zoom.us.app cargo test --release -- --ignored zoom_tables --nocapture`
    #[test]
    #[ignore]
    fn zoom_tables() {
        let app = std::env::var("RECORDEM_APP_BUNDLE").expect("RECORDEM_APP_BUNDLE");
        let t = super::read(std::path::Path::new(&app), &["LN_Accessibility_Video_Render_194601", "LN_Accessibility_Active_Speaker_Name_69648"]);
        eprintln!("{t:?}");
        assert!(t["LN_Accessibility_Video_Render_194601"].iter().any(|v| v == "Video render"));
        assert!(t["LN_Accessibility_Active_Speaker_Name_69648"].iter().any(|v| v == "Talking:"));
    }
}
