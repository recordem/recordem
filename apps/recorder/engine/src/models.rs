// Whisper large-v3-turbo (q5_0, 574 MB) and the Silero VAD (885 KB), downloaded once into models/.
// The VAD runs inside whisper.cpp: silence is cut before decoding, which keeps Whisper from inventing text there.
use futures_util::StreamExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub struct Model {
    pub file: &'static str,
    pub url: &'static str,
    pub size: u64,
}

pub const WHISPER: Model = Model {
    file: "ggml-large-v3-turbo-q5_0.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
    size: 574_041_195,
};
pub const VAD: Model = Model {
    file: "ggml-silero-v5.1.2.bin",
    url: "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin",
    size: 885_098,
};

pub fn path(m: &Model) -> PathBuf {
    crate::config::models_dir().join(m.file)
}

pub fn ready() -> bool {
    [&WHISPER, &VAD].iter().all(|m| std::fs::metadata(path(m)).map(|x| x.len() == m.size).unwrap_or(false))
}

/// Downloads what is missing; `done` counts bytes across both files. Written to *.part and renamed when complete.
pub async fn download(done: Arc<AtomicU64>) -> anyhow::Result<()> {
    std::fs::create_dir_all(crate::config::models_dir())?;
    for m in [&VAD, &WHISPER] {
        let dest = path(m);
        if std::fs::metadata(&dest).map(|x| x.len() == m.size).unwrap_or(false) {
            done.fetch_add(m.size, Ordering::Relaxed);
            continue;
        }
        let part = dest.with_extension("part");
        let res = reqwest::get(m.url).await?.error_for_status()?;
        let mut file = tokio::fs::File::create(&part).await?;
        let mut stream = res.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
            done.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        let got = tokio::fs::metadata(&part).await?.len();
        anyhow::ensure!(got == m.size, "{}: baixou {got} de {} bytes", m.file, m.size);
        tokio::fs::rename(&part, &dest).await?;
    }
    Ok(())
}

pub fn total() -> u64 {
    WHISPER.size + VAD.size
}
