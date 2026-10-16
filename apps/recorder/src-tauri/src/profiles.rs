// The reader profiles in use: the built-in ones (crates/scrape/profiles, compiled in), with the server's on top
// (GET /v1/recorder/profiles). When Meet or Zoom change their window, the fix is a new profile revision on the server:
// every recorder picks it up within hours, no new version. The server's copy is kept on disk (profiles.json, with its
// ETag), so a recorder offline at start still reads with the last one it had; a revision older than the built-in one
// never replaces it.
use recordem_scrape::Profiles;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;

const EVERY: Duration = Duration::from_secs(6 * 3600);

static CURRENT: LazyLock<RwLock<Arc<Profiles>>> = LazyLock::new(|| RwLock::new(Arc::new(with(std::fs::read_to_string(cache()).ok().as_deref()))));

fn cache() -> std::path::PathBuf {
    crate::config::root().join("profiles.json")
}

fn etag_file() -> std::path::PathBuf {
    crate::config::root().join("profiles.etag")
}

/// The built-in profiles, merged with a server document when there is one that parses.
fn with(doc: Option<&str>) -> Profiles {
    let mut p = Profiles::builtin();
    if let Some(doc) = doc
        && let Err(e) = p.merge(doc)
    {
        eprintln!("profiles: the saved server document doesn't parse: {e}");
    }
    p
}

/// The profiles to read with now.
pub fn current() -> Arc<Profiles> {
    CURRENT.read().unwrap().clone()
}

/// Asks the server for its profiles (once now, then every few hours), keeps them when they changed.
pub async fn keep_fresh() {
    loop {
        if let Err(e) = refresh().await {
            eprintln!("profiles: {e}");
        }
        tokio::time::sleep(EVERY).await;
    }
}

async fn refresh() -> anyhow::Result<()> {
    let cfg = crate::config::load();
    let url = format!("{}/v1/recorder/profiles", cfg.api_url.trim_end_matches('/'));
    let mut req = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?.get(&url);
    if let Ok(etag) = std::fs::read_to_string(etag_file()) {
        req = req.header("if-none-match", etag.trim());
    }
    let res = req.send().await?;
    if res.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(());
    }
    let res = res.error_for_status()?;
    let etag = res.headers().get("etag").and_then(|v| v.to_str().ok()).map(String::from);
    let doc = res.text().await?;
    // Only a document that parses replaces the saved one.
    Profiles::builtin().merge(&doc)?;
    std::fs::create_dir_all(crate::config::root())?;
    std::fs::write(cache(), &doc)?;
    if let Some(etag) = etag {
        std::fs::write(etag_file(), etag)?;
    }
    *CURRENT.write().unwrap() = Arc::new(with(Some(&doc)));
    Ok(())
}
