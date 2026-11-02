//! Speech to text on this machine (CON-89). Nothing here is built in: a runtime and a model are
//! fetched when the user asks, in the settings, and run as a program of their own, so neither the
//! build nor the installers need anything for it. The runtime is sherpa-onnx, which runs whisper
//! and parakeet alike.

use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// What a voice note's sender is told when no engine is set up, and the card in the window.
pub use harness_api::MISSING_SPEECH_BACKEND as MISSING_BACKEND;

const RELEASES: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download";
const RUNTIME_VERSION: &str = "v2.0.1";
/// More were measured slower on a loaded machine: a worker losing its core holds the others up.
const THREADS: u32 = 4;
const RATE: usize = 16_000;
/// A test recording stops by itself here.
pub const MAX_TEST_SECONDS: usize = 60;

/// The runtime's archive for this machine, with its digest from the release.
fn runtime_archive() -> Result<(&'static str, &'static str), String> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok(("linux-x64-shared", "2e823376987c3b6960d50726b6560155e2ad92da0b8b3886325773225ec6bdec"))
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Ok(("linux-aarch64-shared-cpu", "a8fc9fb3940d1aa3f83136767742b05068f00253f560d53c4f5aeb75ab4cb350"))
    } else if cfg!(target_os = "macos") {
        Ok(("osx-universal2-shared", "bf6718f708e15390f1da133dd888661a68df270791b178aaf765d4e3f0baa9fe"))
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Ok(("win-x64-shared-MT-Release", "dc703575a68594435f53dec93df775ff4913af851a196c0537163f21d8d2364e"))
    } else {
        Err("no speech runtime is published for this machine".to_string())
    }
}

/// An engine one can choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Whisper,
    Parakeet,
}

impl Engine {
    pub fn id(self) -> &'static str {
        match self {
            Engine::Whisper => "whisper",
            Engine::Parakeet => "parakeet",
        }
    }

    pub fn from_id(id: &str) -> Option<Engine> {
        [Engine::Whisper, Engine::Parakeet].into_iter().find(|e| e.id() == id)
    }
}

/// A model one can choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    pub engine: Engine,
    pub name: &'static str,
    /// What the download comes to, for the choosing.
    pub size: &'static str,
    archive: &'static str,
    /// From the release; older models were published with none.
    digest: Option<&'static str>,
}

pub const MODELS: [Model; 5] = [
    Model { engine: Engine::Whisper, name: "tiny", size: "116 MB", archive: "sherpa-onnx-whisper-tiny", digest: None },
    Model { engine: Engine::Whisper, name: "base", size: "208 MB", archive: "sherpa-onnx-whisper-base", digest: None },
    Model { engine: Engine::Whisper, name: "small", size: "639 MB", archive: "sherpa-onnx-whisper-small", digest: None },
    Model { engine: Engine::Whisper, name: "medium", size: "1.9 GB", archive: "sherpa-onnx-whisper-medium", digest: None },
    // v3 is the multilingual one, Russian among them: v2 gives English-shaped nonsense for it.
    Model { engine: Engine::Parakeet, name: "parakeet-v3", size: "487 MB", archive: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8", digest: Some("5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf") },
];

/// What the runtime comes to, for the choosing.
pub const RUNTIME_SIZE: &str = "28 MB";

pub fn model(name: &str) -> Option<Model> {
    MODELS.into_iter().find(|m| m.name == name)
}

fn speech_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("speech")
}

fn model_dir(data_dir: &Path, model: Model) -> PathBuf {
    speech_dir(data_dir).join("models").join(model.name)
}

fn runtime_dir(data_dir: &Path) -> PathBuf {
    speech_dir(data_dir).join("runtime")
}

fn runtime_program(data_dir: &Path) -> PathBuf {
    let name = if cfg!(windows) { "sherpa-onnx-offline.exe" } else { "sherpa-onnx-offline" };
    runtime_dir(data_dir).join("bin").join(name)
}

/// The files of a model the runtime is given.
fn file_ending(dir: &Path, ending: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(ending)))
}

pub fn runtime_present(data_dir: &Path) -> bool {
    runtime_program(data_dir).exists()
}

pub fn model_present(data_dir: &Path, model: Model) -> bool {
    let dir = model_dir(data_dir, model);
    file_ending(&dir, "tokens.txt").is_some() && file_ending(&dir, "encoder.int8.onnx").is_some() && file_ending(&dir, "decoder.int8.onnx").is_some()
}

/// Everything this model needs is here.
pub fn ready(data_dir: &Path, model: Model) -> bool {
    runtime_present(data_dir) && model_present(data_dir, model)
}

/// How far a download has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Fetching { what: String, have: u64, whole: Option<u64> },
    Unpacking { what: String },
}

/// Fetches what this model needs that is not here yet: the runtime and the model.
pub fn install(data_dir: &Path, model: Model, progress: &mut dyn FnMut(Progress)) -> Result<(), String> {
    install_from(RELEASES, data_dir, model, progress)
}

/// `install`, from a given address.
pub fn install_from(base: &str, data_dir: &Path, model: Model, progress: &mut dyn FnMut(Progress)) -> Result<(), String> {
    if !runtime_present(data_dir) {
        let (platform, digest) = runtime_archive()?;
        let name = format!("sherpa-onnx-{RUNTIME_VERSION}-{platform}");
        let url = format!("{base}/{RUNTIME_VERSION}/{name}.tar.bz2");
        fetch_and_unpack(&url, Some(digest), data_dir, "runtime", &runtime_dir(data_dir), &|_| true, progress)?;
    }
    if !model_present(data_dir, model) {
        let url = format!("{base}/asr-models/{}.tar.bz2", model.archive);
        // Of a model only what is run: its archive also carries larger forms and test sounds.
        let wanted = |name: &str| name.ends_with(".int8.onnx") || name.ends_with("tokens.txt");
        fetch_and_unpack(&url, model.digest, data_dir, model.name, &model_dir(data_dir, model), &wanted, progress)?;
    }
    Ok(())
}

fn fetch_and_unpack(url: &str, digest: Option<&str>, data_dir: &Path, what: &str, to: &Path, wanted: &dyn Fn(&str) -> bool, progress: &mut dyn FnMut(Progress)) -> Result<(), String> {
    let downloads = speech_dir(data_dir).join("downloads");
    std::fs::create_dir_all(&downloads).map_err(|e| format!("creating {}: {e}", downloads.display()))?;
    let part = downloads.join(format!("{what}.part"));
    fetch(url, &part, what, progress)?;
    if let Some(digest) = digest {
        let actual = sha256_of(&part)?;
        if actual != digest {
            let _ = std::fs::remove_file(&part);
            return Err(format!("the {what} download is not what was published, so it was thrown away"));
        }
    }
    progress(Progress::Unpacking { what: what.to_string() });
    let staging = to.with_extension("unpacking");
    let _ = std::fs::remove_dir_all(&staging);
    unpack(&part, &staging, wanted)?;
    let _ = std::fs::remove_dir_all(to);
    std::fs::rename(&staging, to).map_err(|e| format!("putting {} in place: {e}", to.display()))?;
    let _ = std::fs::remove_file(&part);
    Ok(())
}

/// To a file, going on from what an earlier try left.
fn fetch(url: &str, part: &Path, what: &str, progress: &mut dyn FnMut(Progress)) -> Result<(), String> {
    let have = std::fs::metadata(part).map_or(0, |m| m.len());
    // A stalled line is given up on; a slow one is not, so no limit on the whole.
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_connect(Some(Duration::from_secs(30))).timeout_recv_body(None).http_status_as_error(false).build().into();
    let mut request = agent.get(url);
    if have > 0 {
        request = request.header("range", format!("bytes={have}-"));
    }
    let mut response = request.call().map_err(|e| format!("reaching {what}'s address: {e}"))?;
    let status = response.status().as_u16();
    // Asked for more of a file that is whole: what is there is all of it.
    if status == 416 {
        return Ok(());
    }
    if status != 200 && status != 206 {
        return Err(format!("{what}'s address answered {status}"));
    }
    let resumed = if status == 206 { have } else { 0 };
    let whole = response.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).map(|rest| rest + resumed);
    let mut file = if resumed > 0 { std::fs::OpenOptions::new().append(true).open(part) } else { std::fs::File::create(part) }.map_err(|e| format!("writing {}: {e}", part.display()))?;
    let mut reader = response.body_mut().as_reader();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut done = resumed;
    progress(Progress::Fetching { what: what.to_string(), have: done, whole });
    loop {
        let read = reader.read(&mut buffer).map_err(|e| format!("the {what} download stopped at {done} bytes, and goes on from there next time: {e}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(|e| format!("writing {}: {e}", part.display()))?;
        done += read as u64;
        progress(Progress::Fetching { what: what.to_string(), have: done, whole });
    }
    if whole.is_some_and(|whole| done < whole) {
        return Err(format!("the {what} download ended early at {done} bytes, and goes on from there next time"));
    }
    Ok(())
}

fn sha256_of(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| format!("reading {}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// A .tar.bz2, its top folder dropped. Nothing goes outside `to`.
fn unpack(archive: &Path, to: &Path, wanted: &dyn Fn(&str) -> bool) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("reading {}: {e}", archive.display()))?;
    let bytes = bzip2_rs::DecoderReader::new(BufReader::new(file));
    let mut tar = tar::Archive::new(bytes);
    std::fs::create_dir_all(to).map_err(|e| format!("creating {}: {e}", to.display()))?;
    for entry in tar.entries().map_err(|e| format!("reading the archive: {e}"))? {
        let mut entry = entry.map_err(|e| format!("reading the archive: {e}"))?;
        let path = entry.path().map_err(|e| format!("reading the archive: {e}"))?.into_owned();
        let inner: PathBuf = path.components().skip(1).collect();
        let name = inner.to_string_lossy().to_string();
        if name.is_empty() || !wanted(&name) {
            continue;
        }
        // Refuses a path that climbs out.
        entry.unpack_in_inner(to, &inner)?;
    }
    Ok(())
}

trait UnpackInner {
    fn unpack_in_inner(&mut self, to: &Path, inner: &Path) -> Result<(), String>;
}

impl<R: Read> UnpackInner for tar::Entry<'_, R> {
    fn unpack_in_inner(&mut self, to: &Path, inner: &Path) -> Result<(), String> {
        if inner.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            return Err(format!("the archive has a path that leaves its folder: {}", inner.display()));
        }
        let target = to.join(inner);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
        }
        self.unpack(&target).map(|_| ()).map_err(|e| format!("unpacking {}: {e}", inner.display()))
    }
}

/// A Telegram voice note, Opus in Ogg, as 16 kHz mono samples.
pub fn decode(bytes: &[u8]) -> Result<Vec<f32>, String> {
    let mut reader = ogg::PacketReader::new(std::io::Cursor::new(bytes));
    let mut next = || reader.read_packet().map_err(|e| format!("reading the note: {e}"));
    let head = next()?.ok_or("the note is empty")?;
    // OpusHead: the magic, a version, then the channels (RFC 7845).
    if head.data.len() < 10 || &head.data[..8] != b"OpusHead" {
        return Err("the note is not Opus".to_string());
    }
    let channels = head.data[9] as usize;
    if !matches!(channels, 1 | 2) {
        return Err(format!("the note has {channels} channels"));
    }
    next()?.ok_or("the note stops after its header")?;
    let mut decoder = opus_decoder::OpusDecoder::new(48_000, channels).map_err(|e| format!("starting the decoder: {e:?}"))?;
    // The longest Opus frame: 120 ms.
    let mut frame = vec![0.0f32; 5760 * channels];
    let mut mono = Vec::new();
    while let Some(packet) = next()? {
        let samples = decoder.decode_float(&packet.data, &mut frame, false).map_err(|e| format!("decoding the note: {e:?}"))?;
        if channels == 1 {
            mono.extend_from_slice(&frame[..samples]);
        } else {
            mono.extend(frame[..samples * 2].chunks_exact(2).map(|pair| 0.5 * (pair[0] + pair[1])));
        }
    }
    if mono.is_empty() {
        return Err("the note has no sound".to_string());
    }
    // 48 kHz to 16: every third, blended; whisper's own processing dwarfs what this loses.
    Ok(mono.chunks_exact(3).map(|three| (three[0] + three[1] + three[2]) / 3.0).collect())
}

/// One at a time: running side by side is slower than one after the other, on a busy machine.
static RUNNING: Mutex<()> = Mutex::new(());

/// The words of a voice note, in the language they were said in.
pub fn transcribe(data_dir: &Path, chosen: Model, note: &[u8]) -> Result<String, String> {
    transcribe_samples(data_dir, chosen, &decode(note)?)
}

/// The words in 16 kHz mono samples. The model is loaded for each and let go: seconds to load,
/// against gigabytes held by the daemon between notes.
pub fn transcribe_samples(data_dir: &Path, chosen: Model, samples: &[f32]) -> Result<String, String> {
    if !ready(data_dir, chosen) {
        return Err(MISSING_BACKEND.to_string());
    }
    let dir = model_dir(data_dir, chosen);
    let needed = |ending: &str| file_ending(&dir, ending).ok_or(format!("the model has no {ending} file"));
    let (encoder, decoder, tokens) = (needed("encoder.int8.onnx")?, needed("decoder.int8.onnx")?, needed("tokens.txt")?);
    let _running = RUNNING.lock().map_err(|_| "an earlier note broke the engine".to_string())?;
    let sound = speech_dir(data_dir).join(format!("note-{}.wav", std::process::id()));
    write_wav(&sound, samples)?;
    let mut command = Command::new(runtime_program(data_dir));
    command.arg(format!("--tokens={}", tokens.display())).arg(format!("--num-threads={THREADS}"));
    match chosen.engine {
        Engine::Whisper => {
            command.arg(format!("--whisper-encoder={}", encoder.display())).arg(format!("--whisper-decoder={}", decoder.display())).arg("--whisper-task=transcribe");
        }
        Engine::Parakeet => {
            let joiner = needed("joiner.int8.onnx")?;
            command.arg(format!("--encoder={}", encoder.display())).arg(format!("--decoder={}", decoder.display())).arg(format!("--joiner={}", joiner.display()));
        }
    }
    let output = command.arg(&sound).stdin(Stdio::null()).output().map_err(|e| format!("starting the speech runtime: {e}"));
    let _ = std::fs::remove_file(&sound);
    let output = output?;
    let text = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        let tail: Vec<&str> = text.lines().rev().take(4).collect();
        return Err(format!("the speech runtime failed: {}", tail.into_iter().rev().collect::<Vec<_>>().join(" ")));
    }
    words_in(&text).ok_or_else(|| "the speech runtime gave no result".to_string())
}

/// The runtime ends with a line of JSON holding the text.
fn words_in(output: &str) -> Option<String> {
    output.lines().rev().find_map(|line| {
        let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        Some(value["text"].as_str()?.trim().to_string())
    })
}

fn write_wav(path: &Path, samples: &[f32]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    let bytes = samples.len() as u32 * 2;
    let mut out = Vec::with_capacity(44 + bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(RATE as u32).to_le_bytes());
    out.extend_from_slice(&(RATE as u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&bytes.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, out).map_err(|e| format!("writing {}: {e}", path.display()))
}

/// A recording from the microphone, for the settings' test: by a program the machine already has,
/// so the harness links no sound library.
pub struct Recording {
    child: std::process::Child,
    samples: Arc<Mutex<Vec<f32>>>,
    /// The latest level, 0 to 1000.
    level: Arc<AtomicU32>,
    done: Arc<AtomicBool>,
}

/// Programs that record, each as raw 16-bit 16 kHz mono on its output, in order of preference.
fn recorders() -> Vec<(&'static str, Vec<&'static str>)> {
    let mut list = Vec::new();
    if cfg!(target_os = "linux") {
        list.push(("pw-record", vec!["--rate", "16000", "--channels", "1", "--format", "s16", "-"]));
        list.push(("parec", vec!["--rate=16000", "--channels=1", "--format=s16le", "--raw"]));
        list.push(("arecord", vec!["-q", "-f", "S16_LE", "-r", "16000", "-c", "1", "-t", "raw"]));
    }
    if cfg!(target_os = "macos") {
        list.push(("rec", vec!["-q", "-t", "raw", "-r", "16000", "-c", "1", "-b", "16", "-e", "signed", "-"]));
        list.push(("ffmpeg", vec!["-loglevel", "quiet", "-f", "avfoundation", "-i", ":0", "-ar", "16000", "-ac", "1", "-f", "s16le", "-"]));
    }
    list
}

impl Recording {
    pub fn start() -> Result<Recording, String> {
        let mut tried = Vec::new();
        for (program, args) in recorders() {
            match Command::new(program).args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
                Ok(child) => return Ok(Recording::reading(child)),
                Err(_) => tried.push(program),
            }
        }
        if tried.is_empty() {
            Err("recording from the microphone is not built for this system yet".to_string())
        } else {
            Err(format!("nothing here records sound: install one of {}", tried.join(", ")))
        }
    }

    fn reading(mut child: std::process::Child) -> Recording {
        let (samples, level, done) = (Arc::<Mutex<Vec<f32>>>::default(), Arc::<AtomicU32>::default(), Arc::<AtomicBool>::default());
        let mut output = child.stdout.take().expect("piped above");
        let (into, meter, finished) = (samples.clone(), level.clone(), done.clone());
        std::thread::spawn(move || {
            let mut bytes = [0u8; 3200];
            let mut odd: Option<u8> = None;
            while let Ok(read) = output.read(&mut bytes) {
                if read == 0 {
                    break;
                }
                let mut frame = Vec::with_capacity(read / 2 + 1);
                let mut it = bytes[..read].iter().copied();
                if let (Some(low), Some(high)) = (odd.take(), it.clone().next()) {
                    it.next();
                    frame.push(i16::from_le_bytes([low, high]) as f32 / 32768.0);
                }
                while let Some(low) = it.next() {
                    match it.next() {
                        Some(high) => frame.push(i16::from_le_bytes([low, high]) as f32 / 32768.0),
                        None => odd = Some(low),
                    }
                }
                meter.store(level_of(&frame), Ordering::Relaxed);
                let mut all = into.lock().expect("samples");
                if all.len() < MAX_TEST_SECONDS * RATE {
                    all.extend(frame);
                } else {
                    finished.store(true, Ordering::Relaxed);
                }
            }
            finished.store(true, Ordering::Relaxed);
        });
        Recording { child, samples, level, done }
    }

    /// How loud it is now, 0 to 1.
    pub fn level(&self) -> f32 {
        self.level.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn seconds(&self) -> usize {
        self.samples.lock().expect("samples").len() / RATE
    }

    /// It stopped by itself: the limit, or the program ended.
    pub fn finished(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }

    /// Stops, and gives what was heard.
    pub fn stop(mut self) -> Vec<f32> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        std::mem::take(&mut *self.samples.lock().expect("samples"))
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A frame's loudness on a scale that shows speech: its peak's square root, 0 to 1000.
fn level_of(frame: &[f32]) -> u32 {
    let peak = frame.iter().fold(0.0f32, |top, s| top.max(s.abs()));
    (peak.sqrt().min(1.0) * 1000.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_voice_note_decodes_to_16_khz() {
        let note = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/tone.ogg")).unwrap();
        let samples = decode(&note).unwrap();
        // One second of tone, give or take the codec's own lead-in.
        assert!((15_000..=17_000).contains(&samples.len()), "{}", samples.len());
        assert!(samples.iter().any(|s| s.abs() > 0.05), "there is sound in it");
    }

    #[test]
    fn what_is_no_note_says_so() {
        assert!(decode(b"").is_err());
        assert!(decode(b"not an ogg stream at all").is_err());
    }

    /// An archive as the releases have them, by the system's tar.
    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in files {
            let path = dir.path().join("top").join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        let out = dir.path().join("a.tar.bz2");
        let ok = Command::new("tar").arg("cjf").arg(&out).arg("-C").arg(dir.path()).arg("top").status().unwrap();
        assert!(ok.success());
        std::fs::read(out).unwrap()
    }

    fn serve(routes: Vec<(String, Vec<u8>)>, ranges: Arc<Mutex<Vec<String>>>) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", server.server_addr().to_ip().unwrap().port());
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let Some((_, body)) = routes.iter().find(|(path, _)| request.url().ends_with(path.as_str())) else {
                    let _ = request.respond(tiny_http::Response::from_string("no").with_status_code(404));
                    continue;
                };
                let range = request.headers().iter().find(|h| h.field.equiv("range")).map(|h| h.value.as_str().to_string());
                let response = match range {
                    Some(range) => {
                        let from: usize = range.trim_start_matches("bytes=").trim_end_matches('-').parse().unwrap();
                        ranges.lock().unwrap().push(range);
                        tiny_http::Response::from_data(body[from..].to_vec()).with_status_code(206)
                    }
                    None => tiny_http::Response::from_data(body.clone()),
                };
                let _ = request.respond(response.with_chunked_threshold(usize::MAX));
            }
        });
        url
    }

    #[test]
    fn a_model_is_fetched_resumed_unpacked_and_only_what_is_run_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive(&[("tiny-encoder.int8.onnx", b"e"), ("tiny-decoder.int8.onnx", b"d"), ("tiny-tokens.txt", b"t"), ("tiny-encoder.onnx", b"the large form"), ("test_wavs/0.wav", b"a sound")]);
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let url = serve(vec![("/sherpa-onnx-whisper-tiny.tar.bz2".to_string(), bytes.clone())], ranges.clone());
        let tiny = model("tiny").unwrap();
        assert!(!model_present(dir.path(), tiny));
        let part = speech_dir(dir.path()).join("downloads").join("tiny.part");
        std::fs::create_dir_all(part.parent().unwrap()).unwrap();
        std::fs::write(&part, &bytes[..10]).unwrap();
        let mut seen = Vec::new();
        fetch_and_unpack(&format!("{url}/sherpa-onnx-whisper-tiny.tar.bz2"), None, dir.path(), "tiny", &model_dir(dir.path(), tiny), &|n| n.ends_with(".int8.onnx") || n.ends_with("tokens.txt"), &mut |p| seen.push(p)).unwrap();
        assert_eq!(ranges.lock().unwrap().as_slice(), ["bytes=10-"]);
        assert!(matches!(&seen[0], Progress::Fetching { have: 10, whole: Some(whole), .. } if *whole == bytes.len() as u64), "{seen:?}");
        assert!(matches!(seen.last(), Some(Progress::Unpacking { .. })));
        assert!(model_present(dir.path(), tiny) && !part.exists());
        let kept: Vec<String> = std::fs::read_dir(model_dir(dir.path(), tiny)).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(kept.len(), 3, "{kept:?}");
    }

    #[test]
    fn a_download_that_is_not_what_was_published_is_thrown_away() {
        let dir = tempfile::tempdir().unwrap();
        let url = serve(vec![("/x.tar.bz2".to_string(), archive(&[("f", b"1")]))], Default::default());
        let error = fetch_and_unpack(&format!("{url}/x.tar.bz2"), Some("00"), dir.path(), "x", &dir.path().join("to"), &|_| true, &mut |_| {}).unwrap_err();
        assert!(error.contains("thrown away") && !dir.path().join("to").exists(), "{error}");
        assert!(!speech_dir(dir.path()).join("downloads").join("x.part").exists());
    }

    #[test]
    fn with_nothing_installed_it_says_what_the_user_is_to_do() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(transcribe_samples(dir.path(), model("base").unwrap(), &[0.0; 16]).unwrap_err(), MISSING_BACKEND);
    }

    #[test]
    fn the_runtimes_last_line_holds_the_words() {
        assert_eq!(words_in("noise\n{\"lang\": \"\", \"text\": \" привет \", \"tokens\":[]}\n").as_deref(), Some("привет"));
        assert_eq!(words_in("Elapsed: 1\n"), None);
    }

    #[test]
    fn a_recording_reads_its_program_and_shows_the_level() {
        if !cfg!(unix) {
            return;
        }
        // Half a second of loud samples, from a program that then ends.
        let child = Command::new("sh").args(["-c", "i=0; while [ $i -lt 8000 ]; do printf '\\377\\177'; i=$((i+1)); done"]).stdout(Stdio::piped()).spawn().unwrap();
        let recording = Recording::reading(child);
        let start = std::time::Instant::now();
        while !recording.finished() && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(recording.level() > 0.9, "{}", recording.level());
        let samples = recording.stop();
        assert_eq!(samples.len(), 8000);
        assert!(samples.iter().all(|s| *s > 0.9));
    }

    #[test]
    fn a_wav_says_what_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_wav(&path, &[0.0, 1.0, -1.0]).unwrap();
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(bytes.len(), 44 + 6);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16_000);
    }

    /// The real thing: fetches what MULTITOOL_TEST_MODELS need that is not in MULTITOOL_TEST_SPEECH_DIR, and runs
    /// them on MULTITOOL_TEST_WAV, 16 kHz, 16 bits, mono: the words are printed for a person to read.
    #[test]
    #[ignore]
    fn the_runtime_runs_a_real_model() {
        let dir = std::env::var("MULTITOOL_TEST_SPEECH_DIR").unwrap();
        let wav = std::fs::read(std::env::var("MULTITOOL_TEST_WAV").unwrap()).unwrap();
        let samples: Vec<f32> = wav[44..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        for name in std::env::var("MULTITOOL_TEST_MODELS").unwrap().split(',') {
            let wanted = model(name).unwrap();
            install(Path::new(&dir), wanted, &mut |_| {}).unwrap();
            let start = std::time::Instant::now();
            let words = transcribe_samples(Path::new(&dir), wanted, &samples).unwrap();
            eprintln!("{name}: {words:?} in {:?}", start.elapsed());
        }
    }
}
