//! In-app playback of a cached voice note.
//!
//! A voice note is an Ogg/Opus file. [`OpusSource`] reads it one packet at a
//! time, decodes each with the pure-Rust `opus-rs` and hands the samples to
//! `rodio`, whose own audio thread pulls them. Nothing here blocks the caller
//! after [`Player::start`] returns, and decoding never runs on the event loop.
//!
//! The decoder sits behind [`OpusSource`] alone, so a different Opus decoder
//! replaces `opus-rs` by changing that one type.
//!
//! Panics abort the process (`panic = "abort"`), and the audio callback is the
//! one place a panic cannot be recovered from, so the decode path returns an
//! error or ends the stream and never `unwrap`s.
//!
//! The output device lives only while a note plays. It is opened by
//! [`Player::start`] and released by [`Player::stop`], so an idle client holds
//! no audio stream open.

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use ogg::PacketReader;
use opus_rs::OpusDecoder;
use rodio::{
    ChannelCount, DeviceSinkBuilder, DeviceSinkError, MixerDeviceSink, Sample, SampleRate, Source,
};

/// Opus always decodes at 48 kHz, whatever rate the `OpusHead` records; the
/// mixer resamples to the device.
const RATE: u64 = 48_000;

/// The longest frame Opus allows, 120 ms, in samples per channel.
const MAX_FRAME: usize = 5_760;

/// The `OpusHead` fields playback needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpusHead {
    channels: u16,
    /// Samples at 48 kHz the decoder emits before the audio proper starts.
    pre_skip: u16,
}

/// Why a note cannot be played.
#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("cannot read the audio file: {0}")]
    Io(#[from] std::io::Error),
    #[error("not an Ogg stream: {0}")]
    Ogg(#[from] ogg::OggReadError),
    #[error("unsupported audio: {0}")]
    Format(&'static str),
    #[error("opus decoder: {0}")]
    Decoder(&'static str),
    #[error("audio output: {0}")]
    Device(#[from] DeviceSinkError),
}

/// What the player reports about the note it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Playback {
    /// Time played so far; holds while paused.
    pub position: Duration,
    /// Length of the note, taken from the Ogg stream, when it has one.
    pub total: Option<Duration>,
    pub paused: bool,
    /// The note ran to its end. The player still holds it until [`Player::stop`].
    pub finished: bool,
}

/// Plays one voice note at a time on the default output device.
#[derive(Default)]
pub struct Player {
    active: Option<Active>,
}

struct Active {
    player: rodio::Player,
    total: Option<Duration>,
    // Declared last so it drops last: the player stops before its device goes.
    _device: MixerDeviceSink,
}

impl Player {
    /// Stops any note that is playing and starts the file at `path`.
    ///
    /// Opening the file and the output device is quick but is not free; the
    /// samples are decoded on the audio thread, so the call returns once playback
    /// is queued. On error nothing is playing.
    pub fn start(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.active = None;
        let source = OpusSource::open(path)?;
        let total = source.total;

        let mut device = DeviceSinkBuilder::from_default_device()?
            // The default callback prints to stderr, which is the terminal.
            .with_error_callback(|err| tracing::warn!("audio stream error: {err}"))
            .open_sink_or_fallback()?;
        // Same reason: dropping a device otherwise prints a notice.
        device.log_on_drop(false);

        let player = rodio::Player::connect_new(device.mixer());
        player.append(source);
        self.active = Some(Active {
            player,
            total,
            _device: device,
        });
        Ok(())
    }

    /// Pauses a playing note and resumes a paused one. Does nothing once the note
    /// has finished or when none is held.
    pub fn toggle_pause(&self) {
        if let Some(active) = &self.active
            && !active.player.empty()
        {
            if active.player.is_paused() {
                active.player.play();
            } else {
                active.player.pause();
            }
        }
    }

    /// Stops the note and releases the output device.
    pub fn stop(&mut self) {
        self.active = None;
    }

    /// The held note's state, or `None` when nothing was started.
    pub fn snapshot(&self) -> Option<Playback> {
        let active = self.active.as_ref()?;
        let finished = active.player.empty();
        let position = if finished {
            active.total.unwrap_or_else(|| active.player.get_pos())
        } else {
            active.player.get_pos()
        };
        Some(Playback {
            position,
            total: active.total,
            paused: active.player.is_paused(),
            finished,
        })
    }
}

/// An Ogg/Opus file as a stream of interleaved samples.
struct OpusSource {
    reader: PacketReader<BufReader<File>>,
    decoder: OpusDecoder,
    channels: u16,
    total: Option<Duration>,
    /// Decoded samples of the current packet; `pcm[pos..len]` is still to play.
    pcm: Vec<f32>,
    pos: usize,
    len: usize,
    /// Interleaved samples still to discard from the start of the stream.
    skip: usize,
}

impl OpusSource {
    fn open(path: &Path) -> Result<Self, PlayerError> {
        let mut reader = PacketReader::new(BufReader::new(File::open(path)?));
        let first = reader
            .read_packet()?
            .ok_or(PlayerError::Format("empty file"))?;
        let head = parse_head(&first.data)?;
        let total = total_duration(BufReader::new(File::open(path)?), head.pre_skip);
        let channels = usize::from(head.channels);
        let decoder = OpusDecoder::new(48_000, channels).map_err(PlayerError::Decoder)?;
        Ok(Self {
            reader,
            decoder,
            channels: head.channels,
            total,
            pcm: vec![0.0; MAX_FRAME * channels],
            pos: 0,
            len: 0,
            skip: usize::from(head.pre_skip) * channels,
        })
    }

    /// Decodes the next audio packet into `pcm`. `false` means the stream ended.
    fn refill(&mut self) -> bool {
        loop {
            let packet = match self.reader.read_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return false,
                Err(err) => {
                    tracing::warn!("audio stream cut short: {err}");
                    return false;
                }
            };
            if is_container_packet(&packet.data) {
                continue;
            }
            match self.decoder.decode(&packet.data, MAX_FRAME, &mut self.pcm) {
                Ok(frames) => {
                    self.len = (frames * usize::from(self.channels)).min(self.pcm.len());
                    let skipped = self.skip.min(self.len);
                    self.pos = skipped;
                    self.skip -= skipped;
                    return true;
                }
                // One bad packet is a gap in the note, not the end of it.
                Err(err) => tracing::warn!("audio packet dropped: {err}"),
            }
        }
    }
}

impl Iterator for OpusSource {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        while self.pos >= self.len {
            if !self.refill() {
                return None;
            }
        }
        let sample = self.pcm[self.pos];
        self.pos += 1;
        Some(sample)
    }
}

impl Source for OpusSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        ChannelCount::new(self.channels).unwrap_or(ChannelCount::MIN)
    }

    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(48_000).unwrap_or(SampleRate::MIN)
    }

    fn total_duration(&self) -> Option<Duration> {
        self.total
    }
}

/// `OpusHead` and `OpusTags` open every Opus stream and are not audio; feeding
/// either to the decoder is an error.
fn is_container_packet(data: &[u8]) -> bool {
    data.starts_with(b"OpusHead") || data.starts_with(b"OpusTags")
}

fn parse_head(data: &[u8]) -> Result<OpusHead, PlayerError> {
    if !data.starts_with(b"OpusHead") || data.len() < 19 {
        return Err(PlayerError::Format("missing OpusHead"));
    }
    // Version 0 is the only one defined; the high nibble is the compatibility
    // part and a nonzero value means a reader must refuse the stream.
    if data[8] >> 4 != 0 {
        return Err(PlayerError::Format("unknown Opus version"));
    }
    let channels = u16::from(data[9]);
    // Mapping family 0 is mono or stereo, the only layouts the decoder takes
    // and the only ones a voice note uses.
    if !(1..=2).contains(&channels) || data[18] != 0 {
        return Err(PlayerError::Format("unsupported channel layout"));
    }
    Ok(OpusHead {
        channels,
        pre_skip: u16::from_le_bytes([data[10], data[11]]),
    })
}

/// How much of the file's end is read to find its last page: two pages of the
/// largest size Ogg allows, so one of them is whole however the end falls.
const TAIL: u64 = 2 * 65_307;

/// The note's length from the last Ogg page's granule position, minus the
/// pre-skip. Summing decoded packets instead would overstate it by the pre-skip.
/// Only the file's tail is read, so a long note costs no more than a short one.
/// `None` when the stream carries no audio or cannot be read to its end.
fn total_duration<R: Read + Seek>(mut reader: R, pre_skip: u16) -> Option<Duration> {
    let len = reader.seek(SeekFrom::End(0)).ok()?;
    reader
        .seek(SeekFrom::Start(len.saturating_sub(TAIL)))
        .ok()?;
    let mut tail = Vec::new();
    reader.take(TAIL).read_to_end(&mut tail).ok()?;
    // The tail starts anywhere, so try each place a page could begin. A false
    // match inside audio fails the page checksum and the next one is tried.
    let last = (0..tail.len().saturating_sub(3))
        .filter(|&at| tail[at..].starts_with(b"OggS"))
        .find_map(|at| last_granule(&tail[at..]))?;
    let samples = last.saturating_sub(u64::from(pre_skip));
    (samples > 0).then(|| {
        Duration::from_secs(samples / RATE)
            + Duration::from_nanos(samples % RATE * 1_000_000_000 / RATE)
    })
}

/// The granule position of the last page that has one, reading `bytes` as a
/// stream of whole pages. `None` if any page fails to parse.
fn last_granule(bytes: &[u8]) -> Option<u64> {
    let mut reader = PacketReader::new(Cursor::new(bytes));
    // A seek tells the reader that the stream is not read from its start, so it
    // accepts a first page that is not the stream's own.
    reader.seek_bytes(SeekFrom::Start(0)).ok()?;
    let mut last = None;
    loop {
        match reader.read_packet() {
            Ok(Some(packet)) => {
                // A page that ends no packet records "no position" as all ones.
                let granule = packet.absgp_page();
                if granule != u64::MAX {
                    last = Some(granule);
                }
            }
            Ok(None) => return last,
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use ogg::{PacketWriteEndInfo, PacketWriter};
    use opus_rs::{Application, OpusEncoder};

    use super::*;

    const SERIAL: u32 = 7;

    fn head(channels: u8, pre_skip: u16) -> Vec<u8> {
        let mut head = b"OpusHead".to_vec();
        head.extend([1, channels]);
        head.extend(pre_skip.to_le_bytes());
        head.extend(48_000u32.to_le_bytes());
        head.extend(0i16.to_le_bytes());
        head.push(0);
        head
    }

    /// An Ogg stream of the two header packets and `audio`, each packet on its
    /// own page, the last page carrying `last_granule`. The audio packets are
    /// opaque bytes: the duration reads page headers and never the payload.
    fn stream(pre_skip: u16, audio: &[Vec<u8>], last_granule: u64) -> Vec<u8> {
        let mut writer = PacketWriter::new(Vec::new());
        let mut put = |data: &[u8], end, granule| {
            writer
                .write_packet(data.to_vec(), SERIAL, end, granule)
                .expect("write to a Vec cannot fail");
        };
        put(&head(1, pre_skip), PacketWriteEndInfo::EndPage, 0);
        put(b"OpusTags\0\0\0\0\0\0\0\0", PacketWriteEndInfo::EndPage, 0);
        for (i, packet) in audio.iter().enumerate() {
            if i + 1 == audio.len() {
                put(packet, PacketWriteEndInfo::EndStream, last_granule);
            } else {
                put(packet, PacketWriteEndInfo::EndPage, 960 * (i as u64 + 1));
            }
        }
        writer.into_inner()
    }

    #[test]
    fn duration_is_granule_minus_pre_skip() {
        let bytes = stream(312, &[vec![0xFC; 4], vec![0xFC; 4]], 312 + 96_000);
        let total = total_duration(Cursor::new(bytes), 312);
        assert_eq!(total, Some(Duration::from_secs(2)));
    }

    #[test]
    fn duration_keeps_a_fractional_second() {
        // 48 000 samples are a second; 24 samples are 500 microseconds.
        let bytes = stream(0, &[vec![0xFC; 4]], 48_024);
        let total = total_duration(Cursor::new(bytes), 0);
        assert_eq!(total, Some(Duration::from_micros(1_000_500)));
    }

    /// A note longer than the tail that is read still gets its length from the
    /// last page, and the walk reads no more of the file than that tail.
    #[test]
    fn duration_of_a_long_note_comes_from_the_tail() {
        struct Counting(Cursor<Vec<u8>>, std::rc::Rc<std::cell::Cell<usize>>);
        impl Read for Counting {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.0.read(buf)?;
                self.1.set(self.1.get() + n);
                Ok(n)
            }
        }
        impl Seek for Counting {
            fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
                self.0.seek(pos)
            }
        }

        let audio = vec![vec![0xFC; 400]; 2_000];
        let bytes = stream(312, &audio, 312 + 96_000);
        assert!(
            bytes.len() as u64 > 2 * TAIL,
            "the fixture outgrows the tail"
        );

        let read = std::rc::Rc::new(std::cell::Cell::new(0));
        let total = total_duration(Counting(Cursor::new(bytes), read.clone()), 312);

        assert_eq!(total, Some(Duration::from_secs(2)));
        assert!(read.get() as u64 <= TAIL, "read {} bytes", read.get());
    }

    #[test]
    fn duration_is_unknown_without_audio() {
        let bytes = stream(312, &[vec![0xFC; 4]], 100);
        assert_eq!(total_duration(Cursor::new(bytes), 312), None);
        assert_eq!(total_duration(Cursor::new(Vec::new()), 0), None);
    }

    #[test]
    fn container_packets_are_not_audio() {
        assert!(is_container_packet(&head(1, 312)));
        assert!(is_container_packet(b"OpusTags\x08\0\0\0libopus"));
        assert!(!is_container_packet(&[0xFC, 0xFF, 0xFE]));
        assert!(!is_container_packet(&[]));
    }

    #[test]
    fn head_is_read_and_a_tags_packet_is_refused() {
        assert_eq!(
            parse_head(&head(1, 312)).expect("a valid head"),
            OpusHead {
                channels: 1,
                pre_skip: 312
            }
        );
        assert!(parse_head(b"OpusTags\x08\0\0\0libopus").is_err());
        assert!(parse_head(&[0xFC, 0xFF, 0xFE]).is_err());
        assert!(parse_head(&head(1, 0)[..18]).is_err());
        assert!(parse_head(&head(6, 0)).is_err());
        let mut mapped = head(2, 0);
        mapped[18] = 1;
        assert!(parse_head(&mapped).is_err());
        let mut future = head(1, 0);
        future[8] = 0x10;
        assert!(parse_head(&future).is_err());
    }

    /// Encodes a tone, writes it as a real Ogg/Opus file and decodes it back:
    /// the whole decode path, with no audio device.
    #[test]
    fn a_file_decodes_to_its_true_length() {
        const FRAMES: usize = 25; // half a second of 20 ms packets
        let total_duration_of = |samples: usize| {
            Some(Duration::from_nanos(
                samples as u64 * 1_000_000_000 / 48_000,
            ))
        };
        let mut encoder = OpusEncoder::new(48_000, 1, Application::Voip).expect("encoder");
        let pre_skip = u16::try_from(encoder.lookahead()).expect("pre-skip fits");
        let tone: Vec<f32> = (0..960 * FRAMES)
            .map(|i| {
                let t = f32::from(u16::try_from(i).expect("fits")) / 48_000.0;
                (t * 440.0 * std::f32::consts::TAU).sin() * 0.5
            })
            .collect();
        let packets: Vec<Vec<u8>> = tone
            .chunks(960)
            .map(|frame| {
                let mut out = vec![0u8; 1275];
                let n = encoder.encode(frame, 960, &mut out).expect("encode");
                out.truncate(n);
                out
            })
            .collect();
        // The granule counts every decoded sample, pre-skip included.
        let bytes = stream(pre_skip, &packets, 960 * FRAMES as u64);

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("note.ogg");
        std::fs::write(&path, bytes).expect("write fixture");

        let source = OpusSource::open(&path).expect("open");
        let played = 960 * FRAMES - usize::from(pre_skip);
        assert_eq!(source.total, total_duration_of(played));
        let samples: Vec<Sample> = source.collect();
        assert_eq!(samples.len(), played);
        assert!(samples.iter().any(|s| s.abs() > 0.1), "decoded silence");
    }
}
