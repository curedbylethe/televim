//! Stable release checks use bounded platform tools and a small adjacent hint file.
//!
//! The API response is capped at 1 MiB. No archive is read here; downloads are
//! staged in a later update step.

use std::cmp::Ordering;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use tui::app::App;

pub(crate) const RELEASE_URL: &str =
    "https://api.github.com/repos/curedbylethe/televim/releases/latest";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransportError;

pub(crate) trait Transport: Send + Sync {
    fn get(&self, url: &str) -> Result<Vec<u8>, TransportError>;
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PlatformTransport;

impl Transport for PlatformTransport {
    fn get(&self, url: &str) -> Result<Vec<u8>, TransportError> {
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("powershell");
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$ProgressPreference='SilentlyContinue'; (Invoke-WebRequest -UseBasicParsing -TimeoutSec 10 $env:TELEVIM_UPDATE_URL).Content",
            ]);
            command.env("TELEVIM_UPDATE_URL", url);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("curl");
            command.args([
                "-fsSL",
                "--max-time",
                "10",
                "--max-filesize",
                "1048576",
                "--url",
                url,
            ]);
            command
        };

        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| TransportError)?;
        let mut body = Vec::new();
        let stdout = child.stdout.take().ok_or(TransportError)?;
        let read = stdout
            .take((MAX_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut body);
        if read.is_err() || body.len() > MAX_RESPONSE_BYTES {
            let _ = child.kill();
            let _ = child.wait();
            return Err(TransportError);
        }
        let status = child.wait().map_err(|_| TransportError)?;
        if !status.success() || body.is_empty() {
            return Err(TransportError);
        }
        Ok(body)
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Store {
    version: u32,
    pub(crate) dismissed: Option<String>,
    pub(crate) checked: Option<u64>,
}

impl Store {
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
    let Ok(body) = transport.get(url) else {
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
            dismissed: false,
        } => {
            app.set_status(format!(
                "televim {available} available — :update (installed {installed})"
            ));
        }
        Event::Available { .. } | Event::Checked => {}
        Event::Failed => app.flash("could not check for updates"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(Result<Vec<u8>, TransportError>);

    impl Transport for Fake {
        fn get(&self, _url: &str) -> Result<Vec<u8>, TransportError> {
            self.0.clone()
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
    fn store_round_trips_and_corrupt_or_unknown_data_means_empty() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("televim.updates.json");
        let store = Store {
            version: 0,
            dismissed: Some("0.9.0".into()),
            checked: Some(1_762_300_000),
        };
        store.save(&path).expect("write store");
        let loaded = Store::load(&path);
        assert_eq!(loaded.dismissed.as_deref(), Some("0.9.0"));
        assert_eq!(loaded.checked, Some(1_762_300_000));
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
}
