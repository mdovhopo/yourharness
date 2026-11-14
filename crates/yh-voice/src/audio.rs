//! Audio as the models take it: 16 kHz mono, f32 in -1…1.

use std::path::Path;
use std::process::{Command, Stdio};

pub const RATE: u32 = 16_000;

/// `samples` at `from` Hz to 16 kHz: a moving average as the low-pass, then linear steps. Plenty
/// for speech, which the models hear up to 8 kHz.
pub fn resample(samples: &[f32], from: u32) -> Vec<f32> {
    if from == RATE || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = from as f64 / RATE as f64;
    let smoothed: std::borrow::Cow<[f32]> = if ratio > 1. {
        let width = ratio.round().max(1.) as usize;
        let mut out = Vec::with_capacity(samples.len());
        let mut sum = 0f32;
        for (i, s) in samples.iter().enumerate() {
            sum += s;
            if i >= width {
                sum -= samples[i - width];
            }
            out.push(sum / width.min(i + 1) as f32);
        }
        out.into()
    } else {
        samples.into()
    };
    let n = (samples.len() as f64 / ratio).floor() as usize;
    (0..n)
        .map(|i| {
            let at = i as f64 * ratio;
            let (j, frac) = (at.floor() as usize, (at - at.floor()) as f32);
            let a = smoothed[j.min(smoothed.len() - 1)];
            let b = smoothed[(j + 1).min(smoothed.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// Nothing worth sending to a model: under a third of a second, or no louder than a quiet room.
/// Whisper makes words up ("Thank you.") from silence, so silence never reaches it.
pub fn is_silent(samples: &[f32]) -> bool {
    if samples.len() < RATE as usize / 3 {
        return true;
    }
    // The loudest 50 ms window's RMS.
    let loudest = samples.chunks(RATE as usize / 20).map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt()).fold(0f32, f32::max);
    loudest < 0.01
}

/// Any audio file macOS can read (wav, m4a, mp3, caf…) as 16 kHz mono, through `afconvert`.
pub fn read_file(path: &Path) -> Result<Vec<f32>, String> {
    let tmp = std::env::temp_dir().join(format!("yh-voice-{}-{}.wav", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
    let out = Command::new("/usr/bin/afconvert")
        .args(["-f", "WAVE", "-d", "LEI16@16000", "-c", "1"])
        .arg(path)
        .arg(&tmp)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("{} couldn't be read: {}", path.display(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    let bytes = std::fs::read(&tmp).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&tmp);
    pcm16_samples(&bytes?).ok_or_else(|| format!("{} isn't audio afconvert wrote as expected", path.display()))
}

/// 16 kHz mono samples as a 16-bit PCM WAV file's bytes.
pub fn wav16(samples: &[f32]) -> Vec<u8> {
    let data = samples.len() as u32 * 2;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend(b"RIFF");
    out.extend((36 + data).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes()); // PCM
    out.extend(1u16.to_le_bytes()); // mono
    out.extend(RATE.to_le_bytes());
    out.extend((RATE * 2).to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(data.to_le_bytes());
    for s in samples {
        out.extend(((s.clamp(-1., 1.) * 32767.) as i16).to_le_bytes());
    }
    out
}

/// The samples of a 16-bit PCM WAV's `data` chunk.
fn pcm16_samples(wav: &[u8]) -> Option<Vec<f32>> {
    if wav.get(0..4)? != b"RIFF" || wav.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut at = 12;
    while at + 8 <= wav.len() {
        let id = &wav[at..at + 4];
        let len = u32::from_le_bytes(wav[at + 4..at + 8].try_into().ok()?) as usize;
        let body = wav.get(at + 8..(at + 8 + len).min(wav.len()))?;
        if id == b"data" {
            return Some(body.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.).collect());
        }
        at += 8 + len + (len & 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_48k_to_16k() {
        let tone: Vec<f32> = (0..48_000).map(|i| (i as f32 * 440. * std::f32::consts::TAU / 48_000.).sin() * 0.5).collect();
        let out = resample(&tone, 48_000);
        assert_eq!(out.len(), 16_000);
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!((rms - 0.353).abs() < 0.02, "a 440 Hz tone keeps its loudness: {rms}");
    }

    #[test]
    fn silence_is_silent() {
        assert!(is_silent(&vec![0.; 32_000]));
        assert!(is_silent(&vec![0.5; 1_000]), "too short");
        let speech: Vec<f32> = (0..32_000).map(|i| (i as f32 / 20.).sin() * 0.2).collect();
        assert!(!is_silent(&speech));
    }

    #[test]
    fn reads_wav_data_chunk() {
        let mut wav = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0".to_vec();
        wav.extend([0u8; 16]);
        wav.extend(b"data\x04\0\0\0");
        wav.extend(16384i16.to_le_bytes());
        wav.extend((-32768i16).to_le_bytes());
        assert_eq!(pcm16_samples(&wav), Some(vec![0.5, -1.]));
    }

    #[test]
    fn wav16_reads_back() {
        let back = pcm16_samples(&wav16(&[0.5, -0.25, 0.])).unwrap();
        assert!(back.iter().zip([0.5, -0.25, 0.]).all(|(a, b)| (a - b).abs() < 1e-3), "{back:?}");
    }
}
