//! Streaming audio for the preview: a waveform overview computed in one
//! decode pass with constant memory, and PCM chunks around the playhead for
//! playback. Nothing holds the whole file — a multi-GB recording previews like
//! a 3 MB one. Pure Rust (`symphonia`, MPL-2.0 — allowed by deny.toml).

use base64::Engine as _;
use serde::Serialize;
use std::path::Path;

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase};

use crate::path_scope::validate_readable_path;

/// Waveform + format facts for the whole file.
#[derive(Debug, Clone, Serialize)]
pub struct AudioOverview {
    pub sample_rate: u32,
    pub channels: usize,
    pub bits_per_sample: Option<u32>,
    pub duration: f64,
    /// `buckets` × (min, max) of the mono mix, in -1..1.
    pub peaks: Vec<f32>,
}

/// Interleaved f32 PCM for `[start, start + frames/sample_rate)`.
#[derive(Debug, Clone, Serialize)]
pub struct PcmChunk {
    pub sample_rate: u32,
    pub channels: usize,
    pub start: f64,
    pub frames: usize,
    /// Base64 of little-endian f32 samples, interleaved.
    pub data: String,
    pub end_of_stream: bool,
}

struct Opened {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    sample_rate: u32,
    channels: usize,
    bits: Option<u32>,
    meta_duration: Option<f64>,
}

fn open(path: &str) -> Result<Opened, String> {
    let p = Path::new(path);
    validate_readable_path(p)?;
    let file = std::fs::File::open(p).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| format!("Unsupported audio format: {e}"))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or("No audio track found")?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|c| c.audio())
        .ok_or("Audio track has no codec parameters")?
        .clone();
    let track_id = track.id;
    let time_base = track.time_base;
    let sample_rate = params.sample_rate.ok_or("Unknown sample rate")?;
    let channels = params
        .channels
        .as_ref()
        .map(|c| c.count())
        .unwrap_or(1)
        .max(1);
    let meta_duration = match (track.time_base, track.duration) {
        (Some(tb), Some(d)) => tb.calc_duration(d).map(|t| t.as_secs_f64()),
        _ => track.num_frames.map(|n| n as f64 / sample_rate as f64),
    };
    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| format!("Unsupported audio codec: {e}"))?;
    Ok(Opened {
        format,
        decoder,
        track_id,
        time_base,
        sample_rate,
        channels,
        bits: params.bits_per_sample,
        meta_duration,
    })
}

/// Decodes the next packet of the audio track into `out` (interleaved f32).
/// Returns `Ok(Some(start_seconds))` for a decoded packet, `Ok(None)` at end.
fn next_block(o: &mut Opened, out: &mut Vec<f32>) -> Result<Option<Option<f64>>, String> {
    loop {
        let packet = match o.format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => return Ok(None),
            Err(SymError::ResetRequired) => {
                o.decoder.reset();
                continue;
            }
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(None)
            }
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id != o.track_id {
            continue;
        }
        let ts = o
            .time_base
            .and_then(|tb| tb.calc_time(packet.pts))
            .map(|t| t.as_secs_f64());
        match o.decoder.decode(&packet) {
            Ok(buf) => {
                out.resize(buf.samples_interleaved(), 0.0);
                buf.copy_to_slice_interleaved(&mut out[..]);
                return Ok(Some(ts));
            }
            // A corrupt packet is skipped, like every player does.
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// One decode pass → format facts + `buckets` min/max peaks of the mono mix.
/// Memory is bounded: fine peaks are merged pairwise once they pass a cap.
pub fn audio_overview(path: &str, buckets: usize) -> Result<AudioOverview, String> {
    let mut o = open(path)?;
    let buckets = buckets.clamp(16, 20_000);
    const FINE_CAP: usize = 1 << 20;
    let mut fine: Vec<(f32, f32)> = Vec::new();
    let mut per_fine: usize = 256;
    let (mut cur_min, mut cur_max, mut cur_n) = (f32::MAX, f32::MIN, 0usize);
    let mut frames_total: u64 = 0;
    let mut block = Vec::new();
    let ch = o.channels;
    while next_block(&mut o, &mut block)?.is_some() {
        for frame in block.chunks_exact(ch) {
            let v = frame.iter().sum::<f32>() / ch as f32;
            cur_min = cur_min.min(v);
            cur_max = cur_max.max(v);
            cur_n += 1;
            if cur_n == per_fine {
                fine.push((cur_min, cur_max));
                (cur_min, cur_max, cur_n) = (f32::MAX, f32::MIN, 0);
                if fine.len() >= FINE_CAP {
                    fine = fine
                        .chunks(2)
                        .map(|p| {
                            p.iter()
                                .fold((f32::MAX, f32::MIN), |a, b| (a.0.min(b.0), a.1.max(b.1)))
                        })
                        .collect();
                    per_fine *= 2;
                }
            }
        }
        frames_total += (block.len() / ch) as u64;
    }
    if cur_n > 0 {
        fine.push((cur_min, cur_max));
    }
    let mut peaks = Vec::with_capacity(buckets * 2);
    for b in 0..buckets {
        let lo = b * fine.len() / buckets;
        let hi = ((b + 1) * fine.len() / buckets).max(lo + 1).min(fine.len());
        let (mn, mx) = fine
            .get(lo..hi)
            .unwrap_or(&[])
            .iter()
            .fold((0.0f32, 0.0f32), |a, p| (a.0.min(p.0), a.1.max(p.1)));
        peaks.push(mn.clamp(-1.0, 1.0));
        peaks.push(mx.clamp(-1.0, 1.0));
    }
    let decoded = frames_total as f64 / o.sample_rate as f64;
    Ok(AudioOverview {
        sample_rate: o.sample_rate,
        channels: ch,
        bits_per_sample: o.bits,
        duration: if decoded > 0.0 {
            decoded
        } else {
            o.meta_duration.unwrap_or(0.0)
        },
        peaks,
    })
}

/// PCM for `seconds` starting at `start`, resampled (linear) to `out_rate` so
/// any source rate plays through Web Audio (which rejects < 3 kHz buffers).
pub fn audio_pcm_chunk(
    path: &str,
    start: f64,
    seconds: f64,
    out_rate: u32,
) -> Result<PcmChunk, String> {
    let mut o = open(path)?;
    let start = start.max(0.0);
    let seconds = seconds.clamp(0.05, 60.0);
    let out_rate = if out_rate == 0 {
        o.sample_rate
    } else {
        out_rate.clamp(3_000, 384_000)
    };
    if start > 0.0 {
        if let Some(t) = Time::try_from_secs_f64(start) {
            // Formats without a seek index fall back to decoding from the top.
            if o.format
                .seek(
                    SeekMode::Accurate,
                    SeekTo::Time {
                        time: t,
                        track_id: Some(o.track_id),
                    },
                )
                .is_ok()
            {
                o.decoder.reset();
            }
        }
    }
    let ch = o.channels;
    let src_rate = o.sample_rate as f64;
    let want_src = (seconds * src_rate).ceil() as usize + 2;
    let mut src: Vec<f32> = Vec::with_capacity(want_src * ch);
    let mut block = Vec::new();
    let mut eos = false;
    // Position (seconds) of the next decoded frame when packets carry no pts.
    let mut cursor: Option<f64> = None;
    while src.len() / ch < want_src {
        let Some(ts) = next_block(&mut o, &mut block)? else {
            eos = true;
            break;
        };
        let t0 = ts.or(cursor).unwrap_or(0.0);
        let n = block.len() / ch;
        cursor = Some(t0 + n as f64 / src_rate);
        // Drop frames before `start` (seeks land on the preceding packet).
        let skip = if t0 < start {
            (((start - t0) * src_rate).round() as usize).min(n)
        } else {
            0
        };
        src.extend_from_slice(&block[skip * ch..]);
    }
    let src_frames = (src.len() / ch).min(want_src);
    // Read one frame past the window for interpolation, but emit exactly the
    // requested duration so consecutive chunks tile without overlap.
    let requested = (seconds * out_rate as f64).round() as usize;
    let available = ((src_frames as f64) * out_rate as f64 / src_rate).floor() as usize;
    let out_frames = requested.min(available);
    let mut out = Vec::with_capacity(out_frames * ch);
    let step = src_rate / out_rate as f64;
    for i in 0..out_frames {
        let pos = i as f64 * step;
        let i0 = pos.floor() as usize;
        let i1 = (i0 + 1).min(src_frames.saturating_sub(1));
        let frac = (pos - i0 as f64) as f32;
        for c in 0..ch {
            let a = src[i0 * ch + c];
            let b = src[i1 * ch + c];
            out.push(a + (b - a) * frac);
        }
    }
    let mut bytes = Vec::with_capacity(out.len() * 4);
    for s in &out {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    Ok(PcmChunk {
        sample_rate: out_rate,
        channels: ch,
        start,
        frames: out_frames,
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        end_of_stream: eos && src.len() / ch <= want_src,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a 16-bit PCM WAV of a sine at `freq` Hz, `amp` 0..1.
    fn write_wav(path: &std::path::Path, rate: u32, channels: u16, secs: f64, freq: f64, amp: f64) {
        let frames = (rate as f64 * secs) as u32;
        let data_len = frames * channels as u32 * 2;
        let mut b = Vec::with_capacity(44 + data_len as usize);
        b.extend(b"RIFF");
        b.extend((36 + data_len).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(channels.to_le_bytes());
        b.extend(rate.to_le_bytes());
        b.extend((rate * channels as u32 * 2).to_le_bytes());
        b.extend((channels * 2).to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(b"data");
        b.extend(data_len.to_le_bytes());
        for i in 0..frames {
            let v = (amp
                * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin()
                * 32767.0) as i16;
            for _ in 0..channels {
                b.extend(v.to_le_bytes());
            }
        }
        std::fs::write(path, b).unwrap();
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rustic-audio-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    fn decode_f32(b64: &str) -> Vec<f32> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }

    #[test]
    fn overview_reports_format_duration_and_peaks() {
        let p = tmp("tone.wav");
        write_wav(&p, 44_100, 2, 3.0, 440.0, 0.5);
        let o = audio_overview(p.to_str().unwrap(), 300).unwrap();
        assert_eq!(
            (o.sample_rate, o.channels, o.bits_per_sample),
            (44_100, 2, Some(16))
        );
        assert!((o.duration - 3.0).abs() < 0.01, "duration {}", o.duration);
        assert_eq!(o.peaks.len(), 600);
        let max = o.peaks.iter().cloned().fold(0.0f32, f32::max);
        assert!((max - 0.5).abs() < 0.02, "peak {max}");
    }

    #[test]
    fn low_rate_clinical_audio_is_resampled_for_playback() {
        // 2 kHz recordings are rejected by <audio> and by Web Audio buffers.
        let p = tmp("heart.wav");
        write_wav(&p, 2_000, 1, 4.0, 50.0, 0.8);
        let o = audio_overview(p.to_str().unwrap(), 100).unwrap();
        assert_eq!(o.sample_rate, 2_000);
        let c = audio_pcm_chunk(p.to_str().unwrap(), 0.0, 2.0, 48_000).unwrap();
        assert_eq!(c.sample_rate, 48_000);
        assert!(
            (c.frames as i64 - 96_000).abs() <= 48,
            "frames {}",
            c.frames
        );
    }

    #[test]
    fn pcm_chunk_seeks_to_the_requested_time() {
        let p = tmp("seek.wav");
        let rate = 8_000;
        write_wav(&p, rate, 1, 5.0, 1.0, 0.9); // 1 Hz sine: value encodes position
        let c = audio_pcm_chunk(p.to_str().unwrap(), 2.25, 0.5, rate).unwrap();
        let s = decode_f32(&c.data);
        let expected = 0.9 * (2.0 * std::f64::consts::PI * 2.25).sin();
        assert!(
            (s[0] as f64 - expected).abs() < 0.01,
            "first sample {} vs {expected}",
            s[0]
        );
        assert_eq!(c.frames, 4_000);
        assert!(!c.end_of_stream);
        let tail = audio_pcm_chunk(p.to_str().unwrap(), 4.8, 1.0, rate).unwrap();
        assert!(tail.end_of_stream);
        assert!(
            (tail.frames as i64 - 1_600).abs() <= 2,
            "tail frames {}",
            tail.frames
        );
    }

    #[test]
    fn non_audio_files_error_cleanly() {
        let p = tmp("notes.wav");
        std::fs::write(&p, b"not really audio").unwrap();
        assert!(audio_overview(p.to_str().unwrap(), 100).is_err());
    }
}
