#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
// The PoC: every module of PARTICIPANTS.md driven from a terminal on the Mac (over ssh is fine once the terminal's
// host — sshd-keygen-wrapper for ssh — has Accessibility and Screen Recording).
//
//   probe apps                                running call apps and their windows
//   probe dump <bundle-id> [file.json]        the app's Accessibility tree, as a fixture
//   probe scrape <bundle-id|file.json>        what the profiles read from it
//   probe watch <bundle-id> [secs] [dir]      participants and speakers over time → dir/speakers.jsonl, tiles.jsonl
//   probe video <bundle-id> <secs> <dir>      watch + record the call window → dir/window.mp4, crops.json, crops.sh
//   probe name <speakers.jsonl> <turns.json>  voices named by overlap (works anywhere)
//
// Profiles: the built-in ones, merged with $RECORDEM_PROFILES (a server document, {"profiles": [...]}).
use anyhow::{Context, bail};
use recordem_ax::{AxNode, Limits, Scope};
use recordem_scrape::{Profile, Profiles, Scrape};
use recordem_timeline::{Options, Sample, Turn};
use recordem_video::TileAt;
use std::path::{Path, PathBuf};
use std::time::Instant;

const CALL_APPS: [&str; 9] = [
    "us.zoom.xos",
    "com.microsoft.teams2",
    "com.google.Chrome",
    "company.thebrowser.Browser",
    "com.brave.Browser",
    "com.microsoft.edgemac",
    "com.apple.Safari",
    "com.tinyspeck.slackmacgap",
    "com.cisco.webexmeetingsapp",
];

fn profiles() -> anyhow::Result<Profiles> {
    let mut p = Profiles::builtin();
    if let Ok(file) = std::env::var("RECORDEM_PROFILES") {
        let n = p.merge(&std::fs::read_to_string(&file).with_context(|| file.clone())?)?;
        eprintln!("{n} profiles from {file}");
    }
    Ok(p)
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str);
    match arg(0) {
        Some("name") => name(Path::new(arg(1).context("speakers.jsonl")?), Path::new(arg(2).context("turns.json")?)),
        Some("scrape") if arg(1).is_some_and(|a| a.ends_with(".json")) => {
            let tree: AxNode = serde_json::from_str(&std::fs::read_to_string(arg(1).unwrap())?)?;
            let profiles = profiles()?;
            match recordem_scrape::scrape(&profiles.all().iter().collect::<Vec<_>>(), &tree) {
                Some(s) => println!("{}", serde_json::to_string_pretty(&s)?),
                None => println!("no profile recognises a call in it"),
            }
            Ok(())
        }
        #[cfg(target_os = "macos")]
        Some(cmd) => mac::run(cmd, &args[1..]),
        _ => bail!("usage: probe apps | dump <bundle> [file] | scrape <bundle|file> | watch <bundle> [secs] [dir] | video <bundle> <secs> <dir> | name <speakers.jsonl> <turns.json>"),
    }
}

/// speakers.jsonl (one Sample per line) and turns.json (diarization: [{label, start_ms, end_ms}]) → names.
fn name(speakers: &Path, turns: &Path) -> anyhow::Result<()> {
    let samples: Vec<Sample> = std::fs::read_to_string(speakers)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let turns: Vec<Turn> = serde_json::from_str(&std::fs::read_to_string(turns)?)?;
    let opts = Options::default();
    let intervals = recordem_timeline::intervals(&samples, &opts);
    for a in recordem_timeline::assign(&turns, &intervals, &opts) {
        println!("{:<12} {:<30} overlap {:>6.1}s  share {:.0}%", a.label, a.name, a.overlap_ms as f64 / 1000.0, a.share * 100.0);
    }
    Ok(())
}

/// What watch writes: who speaks, per scrape (for naming voices).
struct Log {
    speakers: std::fs::File,
}

impl Log {
    fn new(dir: &Path) -> anyhow::Result<Log> {
        std::fs::create_dir_all(dir)?;
        Ok(Log { speakers: std::fs::File::create(dir.join("speakers.jsonl"))? })
    }

    fn write(&mut self, t_ms: u64, s: &Scrape) -> anyhow::Result<()> {
        use std::io::Write;
        let speaking: Vec<String> = s.participants.iter().filter(|p| p.speaking).map(|p| p.name.clone()).collect();
        writeln!(self.speakers, "{}", serde_json::to_string(&Sample { t_ms, speaking })?)?;
        Ok(())
    }
}

/// One recorded window: the call moves between windows (Meet's tab and its picture-in-picture), so a call is
/// recorded as consecutive segments, one file each, with the tile frames of that window only.
struct Segment {
    index: usize,
    /// Watch time at which this segment's video starts.
    offset_ms: u64,
    /// The recorded window, in points (AX frame).
    frame: recordem_ax::Rect,
    width: u32,
    height: u32,
    tiles: Vec<TileAt>,
    /// Whether this window showed the call, per scrape (segment time).
    shown: Vec<(u64, bool)>,
    /// When the window was read, or known to show something else (segment time): crop_plan's times.
    read: Vec<u64>,
    /// It ended because the call left the window (rather than the recording ending).
    left: bool,
}

/// How often watch scrapes.
const POLL_MS: u64 = 500;

impl Segment {
    fn file(&self) -> String {
        format!("window-{:02}.mp4", self.index)
    }

    fn is(&self, frame: Option<recordem_ax::Rect>) -> bool {
        frame.is_some_and(|f| (f.x - self.frame.x).abs() < 4.0 && (f.y - self.frame.y).abs() < 4.0 && (f.w - self.frame.w).abs() < 4.0)
    }

    /// One scrape: tiles count for this video only if this window is the one showing the call.
    fn observe(&mut self, t_ms: u64, s: &Scrape) {
        let t = t_ms.saturating_sub(self.offset_ms);
        let shown = s.window_shows_call && self.is(s.window_frame);
        self.shown.push((t, shown));
        // Shown but unreadable (another Space): no reading, tiles keep their place.
        if !shown || s.visible {
            self.read.push(t);
        }
        if shown && s.visible {
            for p in &s.participants {
                if let Some(frame) = p.frame {
                    self.tiles.push(TileAt { t_ms: t, name: p.name.clone(), frame });
                }
            }
        }
    }
}

fn summary(s: &Scrape) -> String {
    let people: Vec<String> = s
        .participants
        .iter()
        .map(|p| {
            let flags = [(p.muted == Some(true), " (muted)"), (p.frame.is_none(), " (no tile)")];
            let flags: String = flags.iter().filter(|(on, _)| *on).map(|(_, f)| *f).collect();
            format!("{}{}{}", if p.speaking { "🔊 " } else { "" }, p.name, flags)
        })
        .collect();
    let issues: Vec<String> = s.health.issues.iter().map(|i| serde_json::to_string(i).unwrap_or_default()).collect();
    format!("[{}@{} {:?}{}] {}{}", s.profile, s.revision, s.health.status, if s.visible { "" } else { " not visible" }, people.join(", "), if issues.is_empty() { String::new() } else { format!("  {}", issues.join(" ")) })
}

/// crops.sh: per segment, the window blacked out while it didn't show the call (window-NN-safe.mp4), and each
/// person's tile cut out of it.
fn write_crops(dir: &Path, segments: &[(Segment, u64)]) -> anyhow::Result<PathBuf> {
    let mut sh = String::from("#!/bin/sh\n# Run where ffmpeg is: cuts each participant's tile out of the recorded windows.\nset -e\ncd \"$(dirname \"$0\")\"\n");
    let mut all = Vec::new();
    for (seg, end_ms) in segments {
        // The window changes before a scrape can tell (up to one poll): the end of a segment that stopped because the
        // call left it may already show the other page, and a new one may still show the switch. Both are blacked
        // out.
        let (head, tail) = (300.min(*end_ms), if seg.left { (POLL_MS * 2 + 200).min(*end_ms) } else { 0 });
        let mut shown = seg.shown.clone();
        shown.retain(|(t, _)| *t >= head && *t + tail < *end_ms);
        shown.insert(0, (0, false));
        shown.insert(1, (head, true));
        if tail > 0 {
            shown.push((end_ms - tail, false));
        }
        let hidden = recordem_video::hidden_spans(&shown, *end_ms, 0);
        let mut read = seg.read.clone();
        read.extend([head, end_ms - tail]);
        let tiles: Vec<TileAt> = seg.tiles.iter().filter(|t| t.t_ms >= head && t.t_ms + tail < *end_ms).cloned().collect();
        let plan = recordem_video::crop_plan(&tiles, &read, end_ms - tail, seg.width, seg.height, 0.02);
        let (file, safe) = (seg.file(), seg.file().replace(".mp4", "-safe.mp4"));
        let boxes: Vec<String> = hidden
            .iter()
            .map(|(s, e)| format!("drawbox=c=black:t=fill:enable='between(t,{:.3},{:.3})'", *s as f64 / 1000.0, *e as f64 / 1000.0))
            .collect();
        if boxes.is_empty() {
            sh += &format!("cp {file} {safe}\n");
        } else {
            sh += &format!("ffmpeg -loglevel error -y -i {file} -vf \"{}\" -an {safe}\n", boxes.join(","));
        }
        for (i, s) in plan.iter().enumerate() {
            let name: String = s.name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
            sh += &format!(
                "ffmpeg -loglevel error -y -ss {:.3} -to {:.3} -i {safe} -filter:v crop={}:{}:{}:{} -an '{}_w{:02}_{:02}.mp4'\n",
                s.start_ms as f64 / 1000.0,
                s.end_ms as f64 / 1000.0,
                s.w,
                s.h,
                s.x,
                s.y,
                name,
                seg.index,
                i
            );
        }
        all.push(serde_json::json!({"file": file, "offset_ms": seg.offset_ms, "end_ms": end_ms, "width": seg.width, "height": seg.height, "hidden": hidden, "crops": plan}));
    }
    std::fs::write(dir.join("segments.json"), serde_json::to_string_pretty(&all)?)?;
    let path = dir.join("crops.sh");
    std::fs::write(&path, sh)?;
    Ok(path)
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;

    fn pid(bundle: &str) -> anyhow::Result<i32> {
        recordem_ax::pid_of(bundle).with_context(|| format!("{bundle} is not running"))
    }

    fn trusted() -> anyhow::Result<()> {
        if !recordem_ax::is_trusted() {
            bail!("no Accessibility permission for this process (over ssh: grant it to /usr/libexec/sshd-keygen-wrapper)");
        }
        Ok(())
    }

    /// The profiles for the running app and its version, and a snapshot that reads only what they look at.
    struct Reader<'a> {
        pid: i32,
        profiles: Vec<&'a Profile>,
    }

    impl<'a> Reader<'a> {
        fn new(all: &'a Profiles, bundle: &str) -> anyhow::Result<Reader<'a>> {
            let pid = pid(bundle)?;
            let version = recordem_ax::app_version(pid);
            let profiles = all.for_app(bundle, version.as_deref());
            eprintln!(
                "{bundle} {} → profiles {}",
                version.as_deref().unwrap_or("?"),
                profiles.iter().map(|p| format!("{}@{}", p.id, p.revision)).collect::<Vec<_>>().join(", ")
            );
            Ok(Reader { pid, profiles })
        }

        fn snapshot(&self, scoped: bool) -> Option<AxNode> {
            let window = |w: &AxNode| self.profiles.iter().any(|p| p.wants_window(w));
            let prune = |n: &AxNode| self.profiles.iter().all(|p| p.prunes(n));
            let prune_elsewhere = |n: &AxNode| self.profiles.iter().all(|p| p.prunes_elsewhere(n));
            let scope = if scoped { Scope { window: &window, prune: &prune, prune_elsewhere: &prune_elsewhere } } else { Scope::ALL };
            recordem_ax::snapshot(self.pid, Limits::default(), &scope)
        }

        fn scrape(&self, tree: &AxNode) -> Option<Scrape> {
            recordem_scrape::scrape(&self.profiles, tree)
        }
    }

    pub fn run(cmd: &str, args: &[String]) -> anyhow::Result<()> {
        let arg = |i: usize| args.get(i).map(String::as_str);
        match cmd {
            "apps" => {
                for b in CALL_APPS {
                    if let Some(pid) = recordem_ax::pid_of(b) {
                        println!("{b} (pid {pid})");
                        for w in recordem_ax::windows(pid) {
                            println!("    {:?} {:?}", w.title.unwrap_or_default(), w.frame);
                        }
                    }
                }
                Ok(())
            }
            "dump" => {
                trusted()?;
                let bundle = arg(0).context("bundle id")?;
                let all = profiles()?;
                let reader = Reader::new(&all, bundle)?;
                // A fixture keeps everything (scoped: "dump <bundle> <file> scoped" to see what the recorder reads).
                let started = Instant::now();
                let tree = reader.snapshot(arg(2) == Some("scoped")).context("no tree")?;
                let took = started.elapsed();
                let file = arg(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("{bundle}.json")));
                std::fs::write(&file, serde_json::to_string_pretty(&tree)?)?;
                eprintln!("{} elements in {:.0?} → {}", tree.count(), took, file.display());
                Ok(())
            }
            "scrape" => {
                trusted()?;
                let bundle = arg(0).context("bundle id")?;
                let all = profiles()?;
                let reader = Reader::new(&all, bundle)?;
                for scoped in [false, true] {
                    let started = Instant::now();
                    let tree = reader.snapshot(scoped).context("no tree")?;
                    eprintln!("{}: {} elements, {:.0?}", if scoped { "scoped" } else { "whole app" }, tree.count(), started.elapsed());
                }
                let tree = reader.snapshot(true).context("no tree")?;
                let s = reader.scrape(&tree);
                println!("{}", serde_json::to_string_pretty(&s)?);
                Ok(())
            }
            "watch" | "video" => {
                trusted()?;
                let bundle = arg(0).context("bundle id")?;
                let secs: u64 = arg(1).map(str::parse).transpose()?.unwrap_or(60);
                let dir = PathBuf::from(arg(2).unwrap_or("probe-out"));
                let all = profiles()?;
                let mut reader = Reader::new(&all, bundle)?;
                let pid = reader.pid;
                let mut monitor = recordem_scrape::Monitor::default();
                let mut log = Log::new(&dir)?;
                // The segment being recorded, and the finished ones (with their end, in segment time).
                let mut current: Option<(Segment, recordem_video::WindowRecording)> = None;
                let mut done: Vec<(Segment, u64)> = Vec::new();
                let start = Instant::now();
                let mut last = String::new();
                while start.elapsed().as_secs() < secs {
                    let tick = Instant::now();
                    let tree = reader.snapshot(true);
                    let t_ms = start.elapsed().as_millis() as u64;
                    if let Some(s) = tree.as_ref().and_then(|t| reader.scrape(t)) {
                        for event in monitor.observe(t_ms, &s, None) {
                            eprintln!("health: {}", serde_json::to_string(&event)?);
                            if event == recordem_scrape::Event::Restart {
                                reader = Reader::new(&all, bundle)?;
                            }
                        }
                        if cmd == "video" {
                            // A new segment when the recorded window stopped (it closed) or the call shows in another.
                            let ended = current.as_ref().and_then(|(_, r)| r.ended());
                            let moved = s.window_shows_call && current.as_ref().is_some_and(|(seg, _)| !seg.is(s.window_frame));
                            if ended.is_some() || moved {
                                let (mut seg, rec) = current.take().unwrap();
                                seg.left = true;
                                eprintln!("{} ends: {}", seg.file(), ended.unwrap_or_else(|| "the call moved to another window".into()));
                                let end = t_ms - seg.offset_ms;
                                rec.stop()?;
                                done.push((seg, end));
                            }
                            if current.is_none() && s.window_shows_call {
                                let frame = s.window_frame.context("a visible call has a window")?;
                                let index = done.len();
                                let seg = Segment { index, offset_ms: t_ms, frame, width: 0, height: 0, tiles: Vec::new(), shown: Vec::new(), read: Vec::new(), left: false };
                                match recordem_video::record_window(pid, Some(frame), &dir.join(seg.file())) {
                                    Ok(r) => {
                                        eprintln!("{} starts: {}x{} {:?}", seg.file(), r.width, r.height, s.window_title);
                                        current = Some((Segment { width: r.width, height: r.height, ..seg }, r));
                                    }
                                    Err(e) => eprintln!("can't record that window yet: {e}"),
                                }
                            }
                            if let Some((seg, _)) = current.as_mut() {
                                seg.observe(t_ms, &s);
                            }
                        }
                        log.write(t_ms, &s)?;
                        let line = summary(&s);
                        if line != last {
                            println!("{:>7.1}s  {line}", t_ms as f64 / 1000.0);
                            last = line;
                        }
                    }
                    let took = tick.elapsed();
                    if took.as_millis() > 400 {
                        eprintln!("slow scrape: {took:.0?}");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(POLL_MS).saturating_sub(took));
                }
                if let Some((seg, rec)) = current.take() {
                    let end = start.elapsed().as_millis() as u64 - seg.offset_ms;
                    rec.stop()?;
                    done.push((seg, end));
                }
                if !done.is_empty() {
                    let sh = write_crops(&dir, &done)?;
                    eprintln!("{} segments; {} written", done.len(), sh.display());
                }
                Ok(())
            }
            _ => bail!("unknown command {cmd}"),
        }
    }
}
