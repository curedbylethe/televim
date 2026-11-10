//! Stable release checks use bounded platform tools and a small adjacent hint file.
//!
//! API responses cap at 1 MiB, checksum files at 64 KiB, and release archives at
//! 32 MiB. Archives stream to disk; SHA-256 uses a fixed 64 KiB buffer.

use std::cmp::Ordering;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use serde::{Deserialize, Serialize};
use tui::app::App;

pub(crate) const RELEASE_URL: &str =
    "https://api.github.com/repos/curedbylethe/televim/releases/latest";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_CHECKSUM_BYTES: u64 = 64 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024;
const HASH_BUFFER_BYTES: usize = 64 * 1024;
const CHECK_INTERVAL: u64 = 24 * 60 * 60;
const STORE_VERSION: u32 = 1;

/// A release version in the numeric and pre-release subset this project publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Version {
    major: u32,
    minor: u32,
    patch: u32,
    pre: Option<String>,
}

impl Version {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let (core, pre) = match value.split_once('-') {
            Some((core, pre)) if !pre.is_empty() => (core, Some(pre)),
            Some(_) => return None,
            None => (value, None),
        };
        let mut fields = core.split('.');
        let major = number(fields.next()?)?;
        let minor = number(fields.next()?)?;
        let patch = number(fields.next()?)?;
        if fields.next().is_some() {
            return None;
        }
        if let Some(pre) = &pre
            && (pre.split('.').any(|part| {
                part.is_empty()
                    || !part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                    || (part.len() > 1
                        && part.bytes().all(|byte| byte.is_ascii_digit())
                        && part.starts_with('0'))
            }) || !pre.is_ascii())
        {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            pre: pre.map(str::to_owned),
        })
    }
}

fn number(value: &str) -> Option<u32> {
    if value.is_empty() || (value.len() > 1 && value.starts_with('0')) {
        return None;
    }
    value.parse().ok()
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(left), Some(right)) => compare_pre(left, right),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

fn compare_pre(left: &str, right: &str) -> Ordering {
    let mut left = left.split('.');
    let mut right = right.split('.');
    loop {
        match (left.next(), right.next()) {
            (Some(a), Some(b)) => {
                let order = match (
                    a.bytes().all(|byte| byte.is_ascii_digit()),
                    b.bytes().all(|byte| byte.is_ascii_digit()),
                ) {
                    (true, true) => a.len().cmp(&b.len()).then_with(|| a.cmp(b)),
                    (true, false) => Ordering::Less,
                    (false, true) => Ordering::Greater,
                    (false, false) => a.cmp(b),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

/// Finds GitHub's `tag_name` value without depending on JSON formatting.
pub(crate) fn release_version(body: &[u8]) -> Option<Version> {
    if body.len() > MAX_RESPONSE_BYTES {
        return None;
    }
    let body = std::str::from_utf8(body).ok()?;
    let after_key = body.split_once("\"tag_name\"")?.1;
    let after_colon = after_key.split_once(':')?.1.trim_start();
    let value = after_colon.strip_prefix('"')?.split_once('"')?.0;
    Version::parse(value.strip_prefix('v').unwrap_or(value))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    Available {
        installed: String,
        available: String,
        dismissed: bool,
    },
    Failed,
    Checked,
    Progress(tui::state::update::Progress),
    Ready {
        available: String,
        staged: PathBuf,
    },
    ApplyFailed {
        available: String,
        reason: String,
    },
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransportError;

pub(crate) trait Transport: Send + Sync {
    fn get_bytes(&self, url: &str, cap: u64) -> Result<Vec<u8>, TransportError>;
    fn download(&self, url: &str, path: &Path, cap: u64) -> Result<(), TransportError>;
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PlatformTransport;

impl Transport for PlatformTransport {
    fn get_bytes(&self, url: &str, cap: u64) -> Result<Vec<u8>, TransportError> {
        let mut body = Vec::new();
        Self::run(url, None, cap, Some(&mut body))?;
        if body.is_empty() {
            return Err(TransportError);
        }
        Ok(body)
    }

    fn download(&self, url: &str, path: &Path, cap: u64) -> Result<(), TransportError> {
        Self::run(url, Some(path), cap, None)
    }
}

impl PlatformTransport {
    fn run(
        url: &str,
        path: Option<&Path>,
        cap: u64,
        body: Option<&mut Vec<u8>>,
    ) -> Result<(), TransportError> {
        let reads_body = body.is_some();
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("powershell");
            let script = if path.is_some() {
                r"$r=[Net.HttpWebRequest]::Create($env:TELEVIM_UPDATE_URL); $r.Timeout=10000; $r.ReadWriteTimeout=10000; $s=$null; $f=$null; try {$w=$r.GetResponse(); if($w.ContentLength -gt [long]$env:TELEVIM_UPDATE_CAP){exit 1}; $s=$w.GetResponseStream(); $f=[IO.File]::Create($env:TELEVIM_UPDATE_PATH); $b=New-Object byte[] 65536; [long]$n=0; while(($c=$s.Read($b,0,[int][Math]::Min($b.Length,[long]$env:TELEVIM_UPDATE_CAP-$n+1))) -gt 0){if($n+$c -gt [long]$env:TELEVIM_UPDATE_CAP){exit 1}; $f.Write($b,0,$c); $n+=$c} } finally {if($f){$f.Dispose()}; if($s){$s.Dispose()}; if($w){$w.Dispose()}}"
            } else {
                r"$r=[Net.HttpWebRequest]::Create($env:TELEVIM_UPDATE_URL); $r.Timeout=10000; $r.ReadWriteTimeout=10000; $s=$null; $m=New-Object IO.MemoryStream; try {$w=$r.GetResponse(); if($w.ContentLength -gt [long]$env:TELEVIM_UPDATE_CAP){exit 1}; $s=$w.GetResponseStream(); $b=New-Object byte[] 65536; [long]$n=0; while(($c=$s.Read($b,0,[int][Math]::Min($b.Length,[long]$env:TELEVIM_UPDATE_CAP-$n+1))) -gt 0){if($n+$c -gt [long]$env:TELEVIM_UPDATE_CAP){exit 1}; $m.Write($b,0,$c); $n+=$c}; [Console]::OpenStandardOutput().Write($m.ToArray(),0,[int]$m.Length) } finally {if($s){$s.Dispose()}; if($w){$w.Dispose()}; $m.Dispose()}"
            };
            command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
            command.env("TELEVIM_UPDATE_URL", url);
            command.env("TELEVIM_UPDATE_CAP", cap.to_string());
            if let Some(path) = path {
                command.env("TELEVIM_UPDATE_PATH", path);
            }
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("curl");
            command.args(["-fsSL", "--max-filesize", &cap.to_string()]);
            if path.is_some() {
                command.args([
                    "--connect-timeout",
                    "10",
                    "--speed-limit",
                    "1024",
                    "--speed-time",
                    "10",
                ]);
            } else {
                command.args(["--max-time", "10"]);
            }
            if let Some(path) = path {
                command.arg("-o").arg(path);
            }
            command.arg("--url").arg(url);
            command
        };

        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| TransportError)?;
        let mut body = body;
        if let Some(bytes) = body.as_deref_mut() {
            let stdout = child.stdout.take().ok_or(TransportError)?;
            let read = stdout.take(cap.saturating_add(1)).read_to_end(bytes);
            if read.is_err() || bytes.len() as u64 > cap {
                let _ = child.kill();
                let _ = child.wait();
                return Err(TransportError);
            }
        }
        let status = child.wait().map_err(|_| TransportError)?;
        if !status.success() || reads_body && body.as_ref().is_some_and(|bytes| bytes.is_empty()) {
            return Err(TransportError);
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Store {
    version: u32,
    pub(crate) dismissed: Option<String>,
    pub(crate) checked: Option<u64>,
    pub(crate) latest: Option<String>,
}

impl Store {
    pub(crate) fn cached(&self, installed: &Version) -> Option<Event> {
        let available = Version::parse(self.latest.as_deref()?)?;
        if available <= *installed {
            return None;
        }
        let available = available.to_string();
        Some(Event::Available {
            installed: installed.to_string(),
            dismissed: self.dismissed.as_deref() == Some(available.as_str()),
            available,
        })
    }
    pub(crate) fn load(path: &Path) -> Self {
        let Ok(bytes) = fs::read(path) else {
            return Self::default();
        };
        let Ok(store) = serde_json::from_slice::<Self>(&bytes) else {
            return Self::default();
        };
        if store.version == STORE_VERSION {
            store
        } else {
            Self::default()
        }
    }

    pub(crate) fn save(&self, path: &Path) -> Result<(), String> {
        let mut store = self.clone();
        store.version = STORE_VERSION;
        let bytes = serde_json::to_vec_pretty(&store).map_err(|error| error.to_string())?;
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".{}.tmp", std::process::id()));
        let temp = path.with_file_name(name);
        let result = (|| -> std::io::Result<()> {
            let mut file = fs::File::create(&temp)?;
            restrict_permissions(&temp);
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temp, path)
        })();
        result.map_err(|error| {
            let _ = fs::remove_file(&temp);
            error.to_string()
        })
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        tracing::warn!(%error, "could not restrict update store permissions");
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

pub(crate) fn store_path(config_path: &Path) -> PathBuf {
    config_path.with_extension("updates.json")
}

pub(crate) fn due(last_checked: Option<u64>, now: u64) -> bool {
    last_checked.is_none_or(|last| now.saturating_sub(last) >= CHECK_INTERVAL)
}

pub(crate) fn check(
    transport: &dyn Transport,
    url: &str,
    installed: &Version,
    dismissed: Option<&str>,
) -> Event {
    let Ok(body) = transport.get_bytes(url, MAX_RESPONSE_BYTES as u64) else {
        return Event::Failed;
    };
    let Some(available) = release_version(&body) else {
        return Event::Failed;
    };
    if available <= *installed {
        return Event::Checked;
    }
    let available = available.to_string();
    let is_dismissed = dismissed == Some(available.as_str());
    Event::Available {
        installed: installed.to_string(),
        available,
        dismissed: is_dismissed,
    }
}

pub(crate) fn apply(app: &mut App, event: Event) {
    match event {
        Event::Available {
            installed,
            available,
            dismissed,
        } => {
            app.accept_update(&installed, &available, !dismissed);
        }
        Event::Checked => {}
        Event::Failed => app.flash("could not check for updates"),
        Event::Progress(progress) => app.update_progress(progress),
        Event::Ready { available, staged } => {
            let result = std::env::current_exe()
                .map_err(|_| UpdateError::ReplaceFailed)
                .and_then(|exe| replace(&exe, &staged));
            match result {
                Ok(()) => app.update_installed(&available),
                Err(error) => {
                    if let Some(parent) = staged.parent() {
                        let _ = fs::remove_dir_all(parent);
                    }
                    app.update_failed(error.to_string());
                }
            }
        }
        Event::ApplyFailed { reason, .. } => app.update_failed(reason),
        Event::Cancelled => app.update_cancelled(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdateError {
    ChecksumMismatch,
    ChecksumMissing,
    ArchiveTruncated,
    ArchiveMembers,
    StagingUnwritable,
    StagedBinaryUnusable,
    UnsupportedTarget,
    Transport,
    Managed(Managed),
    ReplaceFailed,
    #[cfg(any(windows, test))]
    RollbackFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Managed {
    Homebrew,
    MacPorts,
    Nix,
    Cargo,
    Snap,
    Flatpak,
    Scoop,
    Winget,
}

impl Managed {
    fn sentence(self) -> &'static str {
        match self {
            Self::Homebrew => "televim was installed by Homebrew. Run brew upgrade televim.",
            Self::MacPorts => "televim was installed by MacPorts. Run sudo port upgrade televim.",
            Self::Nix => "televim was installed by Nix. Run nix profile upgrade televim.",
            Self::Cargo => "televim was installed by cargo. Run cargo install televim.",
            Self::Snap => "televim was installed by snap. Run sudo snap refresh televim.",
            Self::Flatpak => "televim was installed by Flatpak. Run flatpak update.",
            Self::Scoop => "televim was installed by Scoop. Run scoop update televim.",
            Self::Winget => "televim was installed by winget. Run winget upgrade televim.",
        }
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sentence = match self {
            Self::ChecksumMismatch => "The update checksum did not match; nothing was installed.",
            Self::ChecksumMissing => {
                "The update checksum was missing or ambiguous; nothing was installed."
            }
            Self::ArchiveTruncated => {
                "The update archive was incomplete or invalid; nothing was installed."
            }
            Self::ArchiveMembers => {
                "The update archive contained unexpected files; nothing was installed."
            }
            Self::StagingUnwritable => {
                "The update could not be staged; install televim with the README installer."
            }
            Self::StagedBinaryUnusable => {
                "The downloaded update may not match this machine; install televim with the README installer."
            }
            Self::UnsupportedTarget => {
                "Updates support macOS and Linux x86_64/aarch64 and Windows x86_64 only; nothing was installed."
            }
            Self::Transport => "The update download failed; nothing was installed.",
            Self::Managed(manager) => return f.write_str(manager.sentence()),
            Self::ReplaceFailed => {
                "The verified update could not replace televim; nothing was installed. Install manually with the README installer."
            }
            #[cfg(any(windows, test))]
            Self::RollbackFailed => {
                "The verified update could not replace televim, and rollback failed. The previous binary remains beside the executable as .old."
            }
        };
        f.write_str(sentence)
    }
}

fn managed_by(path: &Path) -> Option<Managed> {
    let normalized = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let checks = [
        ("/opt/homebrew/", Managed::Homebrew),
        ("/usr/local/cellar/", Managed::Homebrew),
        ("/usr/local/opt/", Managed::Homebrew),
        ("/opt/local/", Managed::MacPorts),
        ("/nix/store/", Managed::Nix),
        ("/.cargo/bin/", Managed::Cargo),
        ("/snap/", Managed::Snap),
        ("/var/lib/flatpak/", Managed::Flatpak),
        ("/scoop/apps/", Managed::Scoop),
        ("/appdata/local/microsoft/winget/", Managed::Winget),
    ];
    if let Some((_, manager)) = checks
        .iter()
        .find(|(prefix, _)| normalized.contains(prefix))
    {
        return Some(*manager);
    }
    None
}

fn replace(exe: &Path, staged: &Path) -> Result<(), UpdateError> {
    if let Some(manager) = managed_by(exe) {
        return Err(UpdateError::Managed(manager));
    }
    #[cfg(windows)]
    replace_windows(exe, staged)?;
    #[cfg(not(windows))]
    fs::rename(staged, exe).map_err(|_| UpdateError::ReplaceFailed)?;
    if let Some(parent) = staged.parent() {
        let _ = fs::remove_dir(parent);
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn replace_windows(exe: &Path, staged: &Path) -> Result<(), UpdateError> {
    let old = exe.with_extension("exe.old");
    fs::rename(exe, &old).map_err(|_| UpdateError::ReplaceFailed)?;
    if fs::rename(staged, exe).is_err() {
        return match fs::rename(&old, exe) {
            Ok(()) => Err(UpdateError::ReplaceFailed),
            Err(_) => Err(UpdateError::RollbackFailed),
        };
    }
    Ok(())
}

#[cfg(any(windows, test))]
pub(crate) fn remove_old_backup(exe: &Path) {
    let old = exe.with_extension("exe.old");
    if let Err(error) = fs::remove_file(&old) {
        tracing::debug!(%error, path = %old.display(), "could not remove previous executable backup");
    }
}

fn asset(os: &str, arch: &str) -> Result<(&'static str, &'static str), UpdateError> {
    let (target, ext) = match (os, arch) {
        ("macos", "aarch64") => ("aarch64-apple-darwin", "tar.gz"),
        ("macos", "x86_64") => ("x86_64-apple-darwin", "tar.gz"),
        ("linux", "x86_64") => ("x86_64-unknown-linux-gnu", "tar.gz"),
        ("linux", "aarch64") => ("aarch64-unknown-linux-gnu", "tar.gz"),
        ("windows", "x86_64") => ("x86_64-pc-windows-msvc", "zip"),
        _ => return Err(UpdateError::UnsupportedTarget),
    };
    Ok((target, ext))
}

fn checksum(body: &[u8], filename: &str) -> Result<String, UpdateError> {
    if body.len() as u64 > MAX_CHECKSUM_BYTES {
        return Err(UpdateError::ChecksumMissing);
    }
    let mut found = None;
    for line in body.split(|b| *b == b'\n') {
        let line = std::str::from_utf8(line)
            .map_err(|_| UpdateError::ChecksumMissing)?
            .trim_end_matches('\r');
        if line.len() < 66 {
            continue;
        }
        let (hash, suffix) = line.split_at(64);
        let name = suffix
            .strip_prefix("  ")
            .or_else(|| suffix.strip_prefix(" *"));
        let Some(name) = name else {
            continue;
        };
        if name == filename {
            if hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) || found.is_some() {
                return Err(UpdateError::ChecksumMissing);
            }
            found = Some(hash.to_ascii_lowercase());
        }
    }
    found.ok_or(UpdateError::ChecksumMissing)
}

fn digest(path: &Path) -> Result<String, UpdateError> {
    use sha2::Digest as _;

    let mut file = fs::File::open(path).map_err(|_| UpdateError::ArchiveTruncated)?;
    let size = file
        .metadata()
        .map_err(|_| UpdateError::ArchiveTruncated)?
        .len();
    if size == 0 || size > MAX_ARCHIVE_BYTES {
        return Err(UpdateError::ArchiveTruncated);
    }
    let mut hash = sha2::Sha256::new();
    let mut buffer = vec![0u8; HASH_BUFFER_BYTES].into_boxed_slice();
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| UpdateError::ArchiveTruncated)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn verify_digest(path: &Path, expected: &str) -> Result<(), UpdateError> {
    if digest(path)? == expected.to_ascii_lowercase() {
        Ok(())
    } else {
        Err(UpdateError::ChecksumMismatch)
    }
}

fn stage_update(
    transport: &dyn Transport,
    available: &str,
    cancel: &AtomicBool,
    sender: &tokio::sync::mpsc::UnboundedSender<crate::runtime::AppEvent>,
) -> Result<PathBuf, UpdateError> {
    use tui::state::update::Progress;
    let (target, extension) = asset(std::env::consts::OS, std::env::consts::ARCH)?;
    let filename = format!("televim-{available}-{target}.{extension}");
    let base = format!("https://github.com/curedbylethe/televim/releases/download/v{available}/");
    let exe = std::env::current_exe().map_err(|_| UpdateError::StagingUnwritable)?;
    let parent = exe.parent().ok_or(UpdateError::StagingUnwritable)?;
    let stage = parent.join(format!(".televim-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir(&stage).map_err(|_| UpdateError::StagingUnwritable)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = fs::set_permissions(&stage, fs::Permissions::from_mode(0o700));
    }
    let result = (|| {
        if cancel.load(AtomicOrdering::Acquire) {
            return Err(UpdateError::Transport);
        }
        let archive = stage.join(&filename);
        let _ = sender.send(crate::runtime::AppEvent::Update(Event::Progress(
            Progress::Downloading,
        )));
        transport
            .download(&format!("{base}{filename}"), &archive, MAX_ARCHIVE_BYTES)
            .map_err(|_| UpdateError::Transport)?;
        let sums = transport
            .get_bytes(&format!("{base}SHA256SUMS"), MAX_CHECKSUM_BYTES)
            .map_err(|_| UpdateError::Transport)?;
        if cancel.load(AtomicOrdering::Acquire) {
            return Err(UpdateError::Transport);
        }
        let _ = sender.send(crate::runtime::AppEvent::Update(Event::Progress(
            Progress::Verifying,
        )));
        let expected = checksum(&sums, &filename)?;
        verify_digest(&archive, &expected)?;
        if cancel.load(AtomicOrdering::Acquire) {
            return Err(UpdateError::Transport);
        }
        let _ = sender.send(crate::runtime::AppEvent::Update(Event::Progress(
            Progress::Extracting,
        )));
        let binary = extract_binary(&archive, &stage)?;
        if cancel.load(AtomicOrdering::Acquire) {
            return Err(UpdateError::Transport);
        }
        let _ = sender.send(crate::runtime::AppEvent::Update(Event::Progress(
            Progress::Checking,
        )));
        verify_staged_binary(&binary, available)?;
        let _ = fs::remove_file(&archive);
        Ok(binary)
    })();
    match result {
        Ok(path) => Ok(path),
        Err(error) => {
            let _ = fs::remove_dir_all(&stage);
            Err(error)
        }
    }
}

fn extract_binary(archive: &Path, stage: &Path) -> Result<PathBuf, UpdateError> {
    let binary = stage.join(if cfg!(windows) {
        "televim.exe"
    } else {
        "televim"
    });
    #[cfg(windows)]
    {
        let script = r"$z=[IO.Compression.ZipFile]::OpenRead($env:TELEVIM_UPDATE_ARCHIVE); try { $allowed=@('televim.exe','README.md','LICENSE-MIT','LICENSE-APACHE'); foreach($e in $z.Entries){if($e.FullName -match '[/\\]|\.\.' -or $e.FullName -notin $allowed){exit 2}}; $e=$z.GetEntry('televim.exe'); if($null -eq $e){exit 2}; [IO.Compression.ZipFileExtensions]::ExtractToFile($e,$env:TELEVIM_UPDATE_BINARY,$true) } finally {$z.Dispose()}";
        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("TELEVIM_UPDATE_ARCHIVE", archive)
            .env("TELEVIM_UPDATE_BINARY", &binary)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|_| UpdateError::ArchiveTruncated)?;
        if !status.success() {
            return Err(UpdateError::ArchiveMembers);
        }
    }
    #[cfg(unix)]
    {
        let listing = Command::new("tar")
            .args(["-tzf"])
            .arg(archive)
            .output()
            .map_err(|_| UpdateError::ArchiveTruncated)?;
        if !listing.status.success() {
            return Err(UpdateError::ArchiveTruncated);
        }
        let names = String::from_utf8(listing.stdout).map_err(|_| UpdateError::ArchiveMembers)?;
        if names
            .lines()
            .any(|n| !["televim", "README.md", "LICENSE-MIT", "LICENSE-APACHE"].contains(&n))
            || !names.lines().any(|n| n == "televim")
        {
            return Err(UpdateError::ArchiveMembers);
        }
        let status = Command::new("tar")
            .args(["-xzf"])
            .arg(archive)
            .arg("-C")
            .arg(stage)
            .arg("televim")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|_| UpdateError::ArchiveTruncated)?;
        if !status.success() {
            return Err(UpdateError::ArchiveTruncated);
        }
    }
    let metadata = fs::symlink_metadata(&binary).map_err(|_| UpdateError::ArchiveMembers)?;
    if !metadata.file_type().is_file() || metadata.len() == 0 {
        return Err(UpdateError::ArchiveMembers);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(UpdateError::ArchiveMembers);
        }
    }
    Ok(binary)
}

fn verify_staged_binary(binary: &Path, available: &str) -> Result<(), UpdateError> {
    let output = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| UpdateError::StagedBinaryUnusable)?;
    if output.status.success() && output.stdout == format!("televim {available}\n").as_bytes() {
        Ok(())
    } else {
        Err(UpdateError::StagedBinaryUnusable)
    }
}

pub(crate) fn handle(
    app: &mut App,
    config_path: &Path,
    action: tui::state::update::Action,
    cancel: &mut Option<Arc<AtomicBool>>,
    tx: &tokio::sync::mpsc::UnboundedSender<crate::runtime::AppEvent>,
) {
    use tui::state::update::Action;

    match action {
        Action::Open => {
            if !app.open_update() {
                app.flash("no update is available");
            }
        }
        Action::Apply { available } => {
            let managed = std::env::current_exe()
                .ok()
                .and_then(|exe| managed_by(&exe));
            if let Some(manager) = managed {
                app.update_failed(UpdateError::Managed(manager).to_string());
                return;
            }
            if !app.update_begin_apply(&available) {
                return;
            }
            let flag = Arc::new(AtomicBool::new(false));
            *cancel = Some(Arc::clone(&flag));
            let transport = PlatformTransport;
            let sender = tx.clone();
            drop(tokio::spawn(async move {
                let _ = tokio::task::spawn_blocking(move || {
                    let result = stage_update(&transport, &available, &flag, &sender);
                    let event = match result {
                        Ok(staged) => Event::Ready { available, staged },
                        Err(UpdateError::Transport) if flag.load(AtomicOrdering::Acquire) => {
                            Event::Cancelled
                        }
                        Err(error) => {
                            tracing::warn!(%error, "verified update staging failed");
                            Event::ApplyFailed {
                                available,
                                reason: error.to_string(),
                            }
                        }
                    };
                    let _ = sender.send(crate::runtime::AppEvent::Update(event));
                })
                .await;
            }));
        }
        Action::Cancel => {
            if let Some(flag) = cancel {
                flag.store(true, AtomicOrdering::Release);
            }
        }
        Action::Later { available } => {
            let path = store_path(config_path);
            let mut store = Store::load(&path);
            store.dismissed = Some(available);
            if let Err(error) = store.save(&path) {
                tracing::warn!(%error, "could not save update dismissal");
                app.flash("could not save update dismissal");
            }
        }
        Action::Restart => app.confirm_update_restart(),
        Action::InstalledLater => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(Result<Vec<u8>, TransportError>);

    impl Transport for Fake {
        fn get_bytes(&self, _url: &str, _cap: u64) -> Result<Vec<u8>, TransportError> {
            self.0.clone()
        }
        fn download(&self, _url: &str, _path: &Path, _cap: u64) -> Result<(), TransportError> {
            Err(TransportError)
        }
    }

    #[test]
    fn versions_follow_release_order() {
        let ordered = [
            "0.8.0",
            "0.9.0-alpha",
            "0.9.0-beta",
            "0.9.0-rc1",
            "0.9.0",
            "0.10.0",
        ];
        for pair in ordered.windows(2) {
            assert!(
                Version::parse(pair[0]).expect("valid version")
                    < Version::parse(pair[1]).expect("valid version")
            );
        }
        assert!(
            Version::parse("0.9.0-alpha.2").expect("valid version")
                < Version::parse("0.9.0-alpha.10").expect("valid version")
        );
        assert!(
            Version::parse("0.9.0-alpha.2").expect("valid version")
                < Version::parse("0.9.0-alpha.beta").expect("valid version")
        );
        for invalid in ["v0.9.0", "0.9", "0.9.0.1", "0.09.0", "0.9.0-", "0.9.0-01"] {
            assert!(Version::parse(invalid).is_none(), "{invalid} is refused");
        }
    }

    #[test]
    fn release_tag_parser_finds_tag_without_json_spacing_assumptions() {
        assert_eq!(
            release_version(br#"{"tag_name":"v0.9.0"}"#)
                .expect("valid release body")
                .to_string(),
            "0.9.0"
        );
        assert_eq!(
            release_version(br#"{"tag_name" : "v0.10.0"}"#)
                .expect("valid release body")
                .to_string(),
            "0.10.0"
        );
        assert!(release_version(br#"{"tag_name": "draft"}"#).is_none());
        assert!(release_version(&vec![b'a'; MAX_RESPONSE_BYTES + 1]).is_none());
    }

    #[test]
    fn release_asset_map_is_exact_and_checksum_lookup_is_unambiguous() {
        assert_eq!(
            asset("macos", "aarch64"),
            Ok(("aarch64-apple-darwin", "tar.gz"))
        );
        assert_eq!(
            asset("macos", "x86_64"),
            Ok(("x86_64-apple-darwin", "tar.gz"))
        );
        assert_eq!(
            asset("linux", "x86_64"),
            Ok(("x86_64-unknown-linux-gnu", "tar.gz"))
        );
        assert_eq!(
            asset("linux", "aarch64"),
            Ok(("aarch64-unknown-linux-gnu", "tar.gz"))
        );
        assert_eq!(
            asset("windows", "x86_64"),
            Ok(("x86_64-pc-windows-msvc", "zip"))
        );
        assert_eq!(
            asset("windows", "aarch64"),
            Err(UpdateError::UnsupportedTarget)
        );
        assert_eq!(
            asset("linux", "x86_64-musl"),
            Err(UpdateError::UnsupportedTarget)
        );
        let file = "televim-0.9.0-x86_64-unknown-linux-gnu.tar.gz";
        let line = format!("{}  {file}\n", "a".repeat(64));
        assert_eq!(checksum(line.as_bytes(), file), Ok("a".repeat(64)));
        let starred = format!("{} *{file}\n", "A".repeat(64));
        assert_eq!(checksum(starred.as_bytes(), file), Ok("a".repeat(64)));
        assert_eq!(checksum(b"", file), Err(UpdateError::ChecksumMissing));
        assert_eq!(
            checksum(
                format!("{}  {file}\n{}  {file}\n", "a".repeat(64), "b".repeat(64)).as_bytes(),
                file
            ),
            Err(UpdateError::ChecksumMissing)
        );
        assert_eq!(
            checksum(format!("{}  other.zip\n", "a".repeat(64)).as_bytes(), file),
            Err(UpdateError::ChecksumMissing)
        );
    }

    #[test]
    fn archive_digest_streams_and_rejects_mismatch_or_truncation() {
        let dir = tempfile::tempdir().expect("temp dir");
        let archive = dir.path().join("archive");
        fs::write(&archive, b"abc").expect("write archive");
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(verify_digest(&archive, expected), Ok(()));
        assert_eq!(
            verify_digest(&archive, &"0".repeat(64)),
            Err(UpdateError::ChecksumMismatch)
        );
        fs::write(&archive, []).expect("truncate archive");
        assert_eq!(digest(&archive), Err(UpdateError::ArchiveTruncated));
        fs::File::create(&archive)
            .expect("create oversized archive")
            .set_len(MAX_ARCHIVE_BYTES + 1)
            .expect("extend archive");
        assert_eq!(digest(&archive), Err(UpdateError::ArchiveTruncated));
    }

    #[cfg(unix)]
    #[test]
    fn tar_extraction_accepts_only_allowlisted_members_and_checks_version() {
        use std::os::unix::fs::PermissionsExt as _;

        fn archive(dir: &Path, source: &Path) -> PathBuf {
            let archive = dir.join("release.tar.gz");
            let status = Command::new("tar")
                .args(["-czf"])
                .arg(&archive)
                .arg("-C")
                .arg(source)
                .args(["televim", "README.md", "LICENSE-MIT", "LICENSE-APACHE"])
                .status()
                .expect("tar exists");
            assert!(status.success());
            archive
        }

        let dir = tempfile::tempdir().expect("temp dir");
        let source = dir.path().join("source");
        fs::create_dir(&source).expect("source dir");
        let executable = source.join("televim");
        fs::write(&executable, "#!/bin/sh\necho 'televim 0.9.0'\n").expect("write script");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
            .expect("make executable");
        for name in ["README.md", "LICENSE-MIT", "LICENSE-APACHE"] {
            fs::write(source.join(name), "release file").expect("write allowed file");
        }
        let archive = archive(dir.path(), &source);
        let stage = dir.path().join("stage");
        fs::create_dir(&stage).expect("stage dir");
        let binary = extract_binary(&archive, &stage).expect("extract allowlisted binary");
        assert!(binary.is_file());
        assert_eq!(fs::read_dir(&stage).expect("read stage").count(), 1);
        assert_eq!(verify_staged_binary(&binary, "0.9.0"), Ok(()));
        assert_eq!(
            verify_staged_binary(&binary, "0.8.0"),
            Err(UpdateError::StagedBinaryUnusable)
        );

        let unexpected = source.join("unexpected.txt");
        fs::write(&unexpected, "no").expect("write unexpected file");
        let bad = dir.path().join("bad.tar.gz");
        let status = Command::new("tar")
            .args(["-czf"])
            .arg(&bad)
            .arg("-C")
            .arg(&source)
            .arg("televim")
            .arg("unexpected.txt")
            .status()
            .expect("tar exists");
        assert!(status.success());
        let bad_stage = dir.path().join("bad-stage");
        fs::create_dir(&bad_stage).expect("bad stage dir");
        assert_eq!(
            extract_binary(&bad, &bad_stage),
            Err(UpdateError::ArchiveMembers)
        );

        let invalid = dir.path().join("not-gzip");
        fs::write(&invalid, b"not a gzip archive").expect("write invalid archive");
        let invalid_stage = dir.path().join("invalid-stage");
        fs::create_dir(&invalid_stage).expect("invalid stage dir");
        assert_eq!(
            extract_binary(&invalid, &invalid_stage),
            Err(UpdateError::ArchiveTruncated)
        );
    }

    #[cfg(windows)]
    #[test]
    fn zip_extraction_uses_powershell_and_returns_only_the_binary() {
        let dir = tempfile::tempdir().expect("temp dir");
        let source = dir.path().join("source");
        fs::create_dir(&source).expect("source dir");
        fs::copy(
            std::env::current_exe().expect("test binary"),
            source.join("televim.exe"),
        )
        .expect("copy test executable");
        for name in ["README.md", "LICENSE-MIT", "LICENSE-APACHE"] {
            fs::write(source.join(name), "release file").expect("write allowed file");
        }
        let archive = dir.path().join("release.zip");
        let script = "Add-Type -AssemblyName System.IO.Compression.FileSystem; [IO.Compression.ZipFile]::CreateFromDirectory($env:TELEVIM_UPDATE_SOURCE,$env:TELEVIM_UPDATE_ARCHIVE)";
        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("TELEVIM_UPDATE_SOURCE", &source)
            .env("TELEVIM_UPDATE_ARCHIVE", &archive)
            .status()
            .expect("PowerShell exists");
        assert!(status.success());
        let stage = dir.path().join("stage");
        fs::create_dir(&stage).expect("stage dir");
        let binary = extract_binary(&archive, &stage).expect("extract allowlisted executable");
        assert!(binary.is_file());
        assert_eq!(fs::read_dir(&stage).expect("read stage").count(), 1);
        assert_eq!(
            verify_staged_binary(&binary, "0.9.0"),
            Err(UpdateError::StagedBinaryUnusable)
        );
    }

    #[test]
    fn daily_check_and_transport_failure_are_pure() {
        assert!(due(None, 10));
        assert!(!due(Some(10), 10 + CHECK_INTERVAL - 1));
        assert!(due(Some(10), 10 + CHECK_INTERVAL));
        assert!(!due(Some(100), 10));
        assert_eq!(
            check(
                &Fake(Err(TransportError)),
                "unused",
                &Version::parse("0.8.0").expect("valid installed version"),
                None
            ),
            Event::Failed
        );
    }

    #[test]
    fn later_persists_dismissal_without_dropping_check_time() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = dir.path().join("televim.toml");
        let path = store_path(&config);
        let store = Store {
            checked: Some(123),
            ..Store::default()
        };
        store.save(&path).expect("initial store saves");

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        handle(
            &mut App::new(),
            &config,
            tui::state::update::Action::Later {
                available: "0.9.0".to_owned(),
            },
            &mut None,
            &tx,
        );

        let saved = Store::load(&path);
        assert_eq!(saved.dismissed.as_deref(), Some("0.9.0"));
        assert_eq!(saved.checked, Some(123));
    }

    #[test]
    fn store_round_trips_and_corrupt_or_unknown_data_means_empty() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("televim.updates.json");
        let store = Store {
            version: 0,
            dismissed: Some("0.9.0".into()),
            checked: Some(1_762_300_000),
            latest: Some("0.9.0".into()),
        };
        store.save(&path).expect("write store");
        let loaded = Store::load(&path);
        assert_eq!(loaded.dismissed.as_deref(), Some("0.9.0"));
        assert_eq!(loaded.checked, Some(1_762_300_000));
        assert_eq!(loaded.latest.as_deref(), Some("0.9.0"));
        assert_eq!(
            loaded.cached(&Version::parse("0.8.0").expect("valid version")),
            Some(Event::Available {
                installed: "0.8.0".into(),
                available: "0.9.0".into(),
                dismissed: true,
            })
        );
        assert!(
            loaded
                .cached(&Version::parse("0.9.0").expect("valid version"))
                .is_none()
        );
        fs::write(&path, b"bad json").expect("replace store");
        assert!(Store::load(&path).checked.is_none());
        fs::write(&path, br#"{"version":2,"checked":42}"#).expect("replace schema");
        assert!(Store::load(&path).checked.is_none());
    }

    #[test]
    fn dismissed_version_is_still_available_without_reprompting() {
        let fake = Fake(Ok(br#"{"tag_name":"v0.9.0"}"#.to_vec()));
        let event = check(
            &fake,
            "unused",
            &Version::parse("0.8.0").expect("valid installed version"),
            Some("0.9.0"),
        );
        assert!(matches!(
            event,
            Event::Available {
                dismissed: true,
                ..
            }
        ));
    }

    #[test]
    fn an_update_replaces_only_the_executable() {
        let dir = tempfile::tempdir().expect("temp dir");
        let exe = dir.path().join("televim");
        let staged_dir = dir.path().join("stage");
        fs::create_dir(&staged_dir).expect("stage dir");
        let staged = staged_dir.join("televim");
        let config = dir.path().join("televim.toml");
        let session = dir.path().join("session");
        let key = dir.path().join("session.key");
        let accounts = dir.path().join("televim.accounts.json");
        let draft = dir.path().join("draft");
        let history = dir.path().join("history");
        fs::write(&exe, b"old").expect("old executable");
        fs::write(&staged, b"verified new binary").expect("staged executable");
        fs::write(&config, b"config").expect("config");
        fs::write(&session, b"session").expect("session");
        fs::write(&key, b"key").expect("key");
        fs::write(&accounts, b"accounts").expect("accounts");
        fs::write(&draft, b"draft").expect("draft");
        fs::write(&history, b"history").expect("history");
        replace(&exe, &staged).expect("replace executable");
        assert_eq!(
            fs::read(&exe).expect("new executable"),
            b"verified new binary"
        );
        assert_eq!(fs::read(config).expect("config remains"), b"config");
        assert_eq!(fs::read(session).expect("session remains"), b"session");
        assert_eq!(fs::read(key).expect("key remains"), b"key");
        assert_eq!(fs::read(accounts).expect("accounts remain"), b"accounts");
        assert_eq!(fs::read(draft).expect("draft remains"), b"draft");
        assert_eq!(fs::read(history).expect("history remains"), b"history");
        #[cfg(not(windows))]
        assert!(!exe.with_extension("old").exists());
    }

    #[test]
    fn managed_prefixes_normalize_both_separator_styles() {
        let cases = [
            ("/opt/homebrew/bin/televim", Managed::Homebrew),
            ("C:\\opt\\homebrew\\bin\\televim.exe", Managed::Homebrew),
            ("/usr/local/Cellar/televim/bin/televim", Managed::Homebrew),
            ("/opt/local/bin/televim", Managed::MacPorts),
            ("/nix/store/hash-televim/bin/televim", Managed::Nix),
            ("/home/user/.cargo/bin/televim", Managed::Cargo),
            ("/snap/bin/televim", Managed::Snap),
            ("/var/lib/flatpak/exports/bin/televim", Managed::Flatpak),
            (
                "C:\\Users\\me\\scoop\\apps\\televim\\current\\televim.exe",
                Managed::Scoop,
            ),
            (
                "C:\\Users\\me\\AppData\\Local\\Microsoft\\WinGet\\Links\\televim.exe",
                Managed::Winget,
            ),
        ];
        for (path, expected) in cases {
            assert_eq!(managed_by(Path::new(path)), Some(expected), "{path}");
        }
        assert_eq!(managed_by(Path::new("/home/user/.local/bin/televim")), None);
    }

    #[cfg(unix)]
    #[test]
    fn read_only_executable_directory_refuses_without_changing_original() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("temp dir");
        let exe = dir.path().join("televim");
        let staged = dir.path().join("staged");
        fs::write(&exe, b"original").expect("original executable");
        fs::write(&staged, b"verified").expect("staged executable");
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555))
            .expect("make directory read-only");
        assert_eq!(replace(&exe, &staged), Err(UpdateError::ReplaceFailed));
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755))
            .expect("restore directory permissions");
        assert_eq!(fs::read(&exe).expect("original remains"), b"original");
        assert_eq!(
            fs::read(&staged).expect("verified stage remains"),
            b"verified"
        );
    }

    #[test]
    fn windows_second_rename_failure_rolls_back_and_backup_sweep_is_best_effort() {
        let dir = tempfile::tempdir().expect("temp dir");
        let exe = dir.path().join("televim.exe");
        fs::write(&exe, b"old").expect("old executable");
        let missing_stage = dir.path().join("missing.exe");
        assert_eq!(
            replace_windows(&exe, &missing_stage),
            Err(UpdateError::ReplaceFailed)
        );
        assert_eq!(fs::read(&exe).expect("rollback restored exe"), b"old");
        let staged = dir.path().join("staged.exe");
        fs::write(&staged, b"new").expect("staged executable");
        replace_windows(&exe, &staged).expect("windows replacement");
        let old = exe.with_extension("exe.old");
        assert_eq!(fs::read(&exe).expect("new executable"), b"new");
        assert_eq!(
            fs::read(&old).expect("backup remains until next launch"),
            b"old"
        );
        remove_old_backup(&exe);
        assert!(!old.exists());
        remove_old_backup(&exe);
    }
}
