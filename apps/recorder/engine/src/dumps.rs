// What the call's window looked like when its reader broke: the last good snapshot of the stage and the broken one,
// side by side in the recording's folder (reader/<t_ms>-good.json.gz, -bad.json.gz), three pairs per recording at
// most. They stay on this Mac; with "Help fix call readers" on, they go to the server after the call with the audio,
// for whoever fixes the profile. Only the stage (the tiles) is in them, and e-mail addresses are blanked.
use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_PAIRS: usize = 3;

fn folder(dir: &Path) -> PathBuf {
    dir.join("reader")
}

/// Keeps a pair (or the broken one alone, when nothing good was seen yet). Best effort: a failure only loses it.
pub fn keep(dir: &Path, t_ms: u64, good: Option<&recordem_ax::AxNode>, bad: &recordem_ax::AxNode) {
    let f = folder(dir);
    let pairs = std::fs::read_dir(&f).map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().ends_with("-bad.json.gz")).count()).unwrap_or(0);
    if pairs >= MAX_PAIRS || std::fs::create_dir_all(&f).is_err() {
        return;
    }
    let write = |name: String, tree: &recordem_ax::AxNode| -> anyhow::Result<()> {
        let json = blank_emails(&serde_json::to_string(tree)?);
        let mut gz = flate2::write::GzEncoder::new(std::fs::File::create(f.join(name))?, flate2::Compression::default());
        gz.write_all(json.as_bytes())?;
        gz.finish()?;
        Ok(())
    };
    let t = t_ms.min(999_999_999);
    if let Some(g) = good
        && let Err(e) = write(format!("{t}-good.json.gz"), g)
    {
        eprintln!("reader dump: {e}");
    }
    if let Err(e) = write(format!("{t}-bad.json.gz"), bad) {
        eprintln!("reader dump: {e}");
    }
}

fn blank_emails(s: &str) -> String {
    static EMAIL: std::sync::LazyLock<recordem_scrape::Regex> =
        std::sync::LazyLock::new(|| recordem_scrape::Regex::new(r"[\w.+-]+@[\w-]+(\.[\w-]+)+").unwrap());
    EMAIL.replace_all(s, "someone@example.com").into_owned()
}

/// The dumps of a recording, to send: (file name, bytes).
pub fn all(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let Ok(d) = std::fs::read_dir(folder(dir)) else { return Vec::new() };
    let mut out: Vec<(String, Vec<u8>)> = d
        .flatten()
        .filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).ok()?)))
        .filter(|(n, _)| n.ends_with(".json.gz"))
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn emails_are_blanked() {
        assert_eq!(super::blank_emails(r#"{"value":"Participando como ana.souza+x@mail.example.org"}"#), r#"{"value":"Participando como someone@example.com"}"#);
    }
}
