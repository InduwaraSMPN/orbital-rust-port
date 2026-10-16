//! Sound port of `SoundHelper.cs` — WAV synthesis is verbatim (mono 44100 Hz 16-bit
//! PCM sine with exponential decay: popup 820 Hz/35 ms/decay 55/vol 0.13, action
//! 1100 Hz/55 ms/decay 45/vol 0.15), fire-and-forget rodio sink replaces SoundPlayer.

/// C# `SoundHelper.GenerateWav` — returns a complete RIFF/WAVE byte vector.
pub fn generate_wav(frequency_hz: f64, duration_ms: i32, decay_rate: f64, volume: f64) -> Vec<u8> {
    const SAMPLE_RATE: i32 = 44100;
    const BITS_PER_SAMPLE: i32 = 16;
    const CHANNELS: i32 = 1;

    let sample_count = (SAMPLE_RATE * duration_ms / 1000).max(0);
    let data_size = sample_count * (BITS_PER_SAMPLE / 8);

    let mut out = Vec::with_capacity(44 + data_size as usize);

    // ── RIFF header ──
    out.extend_from_slice(b"RIFF");
    push_u32_le(&mut out, (36 + data_size) as u32); // ChunkSize
    out.extend_from_slice(b"WAVE");

    // ── fmt sub-chunk ──
    out.extend_from_slice(b"fmt ");
    push_u32_le(&mut out, 16); // SubChunk1Size (PCM)
    push_u16_le(&mut out, 1); // AudioFormat = PCM
    push_u16_le(&mut out, CHANNELS as u16); // NumChannels
    push_u32_le(&mut out, SAMPLE_RATE as u32); // SampleRate
    push_u32_le(
        &mut out,
        (SAMPLE_RATE * CHANNELS * (BITS_PER_SAMPLE / 8)) as u32,
    ); // ByteRate
    push_u16_le(&mut out, (CHANNELS * (BITS_PER_SAMPLE / 8)) as u16); // BlockAlign
    push_u16_le(&mut out, BITS_PER_SAMPLE as u16); // BitsPerSample

    // ── data sub-chunk ──
    out.extend_from_slice(b"data");
    push_u32_le(&mut out, data_size as u32);

    // ── PCM samples: sine wave with exponential decay ──
    for i in 0..sample_count {
        let t = i as f64 / SAMPLE_RATE as f64;
        let envelope = (-t * decay_rate).exp();
        let sample = (2.0 * std::f64::consts::PI * frequency_hz * t).sin() * envelope * volume;
        // C# `(short)Math.Clamp(sample * short.MaxValue, short.MinValue, short.MaxValue)`
        // — clamp to double first, then truncate toward zero on the cast.
        let pcm = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
        push_i16_le(&mut out, pcm);
    }

    out
}

fn push_u16_le(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32_le(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_i16_le(out: &mut Vec<u8>, value: i16) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// C# `PlayPopupSound` — gated on `SoundEnabled`; audio failure must never crash.
pub fn play_popup(sound_enabled: bool) {
    if !sound_enabled {
        return;
    }
    let wav = generate_wav(820.0, 35, 55.0, 0.13);
    fire_and_forget(wav);
}

/// C# `PlayActionSound`.
pub fn play_action(sound_enabled: bool) {
    if !sound_enabled {
        return;
    }
    let wav = generate_wav(1100.0, 55, 45.0, 0.15);
    fire_and_forget(wav);
}

/// C# `new SoundPlayer(ms).Play()` — non-blocking; the device sink and player are
/// moved into the playback thread and kept alive until the tone finishes, like the
/// `MemoryStream` the source keeps alive behind `SoundPlayer`.
fn fire_and_forget(wav: Vec<u8>) {
    std::thread::spawn(move || {
        // C# `Play()` failure path: swallow everything — audio must never crash the app.
        let Ok(device_sink) = rodio::DeviceSinkBuilder::open_default_sink() else {
            tracing::debug!("no default audio output device for the gesture sound");
            return;
        };
        let player = rodio::Player::connect_new(device_sink.mixer());
        let cursor = std::io::Cursor::new(wav);
        let Ok(source) = rodio::Decoder::new(cursor) else {
            tracing::debug!("the generated WAV could not be decoded");
            return;
        };
        player.append(source);
        // Keeps the device sink and player alive until the tone finishes.
        player.sleep_until_end();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }

    fn read_u16_le(bytes: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
    }

    #[test]
    fn popup_wav_header_matches_the_csharp_layout() {
        let wav = generate_wav(820.0, 35, 55.0, 0.13);

        let sample_count = 44100 * 35 / 1000;
        let data_size = sample_count * 2;

        assert_eq!(wav.len(), 44 + data_size as usize);

        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(read_u32_le(&wav, 4), (36 + data_size) as u32);
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(read_u32_le(&wav, 16), 16); // SubChunk1Size (PCM)
        assert_eq!(read_u16_le(&wav, 20), 1); // AudioFormat = PCM
        assert_eq!(read_u16_le(&wav, 22), 1); // NumChannels
        assert_eq!(read_u32_le(&wav, 24), 44100); // SampleRate
        assert_eq!(read_u32_le(&wav, 28), 88200); // ByteRate
        assert_eq!(read_u16_le(&wav, 32), 2); // BlockAlign
        assert_eq!(read_u16_le(&wav, 34), 16); // BitsPerSample
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(read_u32_le(&wav, 40), data_size as u32);
    }

    #[test]
    fn action_wav_header_matches_the_csharp_layout() {
        let wav = generate_wav(1100.0, 55, 45.0, 0.15);

        let data_size = (44100 * 55 / 1000) * 2;

        assert_eq!(wav.len(), 44 + data_size as usize);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(read_u32_le(&wav, 4), (36 + data_size) as u32);
        assert_eq!(read_u32_le(&wav, 40), data_size as u32);
    }

    #[test]
    fn first_sample_of_a_popup_is_zero() {
        let wav = generate_wav(820.0, 35, 55.0, 0.13);
        // sin(2*pi*820*0) == 0, so the first 16-bit LE sample is 0.
        assert_eq!(read_u16_le(&wav, 44), 0);
    }

    #[test]
    fn samples_follow_the_decaying_sine_formula() {
        let wav = generate_wav(820.0, 35, 55.0, 0.13);
        let sample_count = 44100 * 35 / 1000;

        for i in [1usize, 7, 100, 500, sample_count as usize - 1] {
            let t = i as f64 / 44100.0;
            let expected = ((2.0 * std::f64::consts::PI * 820.0 * t).sin()
                * (-t * 55.0).exp()
                * 0.13
                * 32767.0)
                .clamp(-32768.0, 32767.0) as i16;
            let actual = read_u16_le(&wav, 44 + i * 2) as i16;
            assert_eq!(actual, expected, "sample {i} mismatch");
        }
    }

    #[test]
    fn zero_duration_produces_a_header_only_wav() {
        let wav = generate_wav(820.0, 0, 55.0, 0.13);

        assert_eq!(wav.len(), 44);
        assert_eq!(read_u32_le(&wav, 36 + 4), 0);
    }
}
