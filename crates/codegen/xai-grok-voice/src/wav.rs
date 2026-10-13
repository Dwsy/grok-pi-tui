//! RIFF/WAVE wrapper for a PCM16 mono clip. Batch STT identifies the sample format from the container, so the raw
//! frames get a 44-byte canonical header; nothing is resampled.

/// RIFF header (12) + PCM `fmt ` chunk (24) + `data` chunk header (8).
pub(crate) const WAV_HEADER_LEN: usize = 44;

const CHANNELS: u16 = 1;
const BITS_PER_SAMPLE: u16 = 16;
/// WAVE_FORMAT_PCM.
const FORMAT_PCM: u16 = 1;

/// Wraps little-endian PCM16 mono frames at `sample_rate` Hz in a RIFF/WAVE container.
///
/// A trailing odd byte (half a sample) is dropped so the data chunk stays frame-aligned. Clips over `u32::MAX - 36`
/// bytes cannot be represented in RIFF; the pipeline's clip cap keeps recordings far below that.
pub(crate) fn encode_pcm16_mono_wav(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    let data_len = pcm.len() & !1;
    let data = pcm.get(..data_len).unwrap_or_default();
    let data_len_u32 = u32::try_from(data_len).unwrap_or(u32::MAX - 36);
    let block_align = CHANNELS * (BITS_PER_SAMPLE / 8);
    let byte_rate = sample_rate.saturating_mul(u32::from(block_align));

    let mut out = Vec::with_capacity(WAV_HEADER_LEN + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36u32.saturating_add(data_len_u32)).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&FORMAT_PCM.to_le_bytes());
    out.extend_from_slice(&CHANNELS.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len_u32.to_le_bytes());
    out.extend_from_slice(data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_bytes_for_four_samples_at_16khz() {
        let pcm = [0x01, 0x00, 0xff, 0x7f, 0x00, 0x80, 0x00, 0x00];
        let wav = encode_pcm16_mono_wav(&pcm, 16_000);

        let expected_header: [u8; WAV_HEADER_LEN] = [
            b'R', b'I', b'F', b'F', // ChunkID
            44, 0, 0, 0, // ChunkSize = 36 + 8
            b'W', b'A', b'V', b'E', // Format
            b'f', b'm', b't', b' ', // Subchunk1ID
            16, 0, 0, 0, // Subchunk1Size
            1, 0, // AudioFormat = PCM
            1, 0, // NumChannels = 1
            0x80, 0x3e, 0, 0, // SampleRate = 16000
            0x00, 0x7d, 0, 0, // ByteRate = 32000
            2, 0, // BlockAlign
            16, 0, // BitsPerSample
            b'd', b'a', b't', b'a', // Subchunk2ID
            8, 0, 0, 0, // Subchunk2Size
        ];
        assert_eq!(
            expected_header.as_slice(),
            wav.get(..WAV_HEADER_LEN).unwrap()
        );
        assert_eq!(pcm.as_slice(), wav.get(WAV_HEADER_LEN..).unwrap());
        assert_eq!(WAV_HEADER_LEN + pcm.len(), wav.len());
    }

    #[test]
    fn empty_clip_is_a_header_only_file() {
        let wav = encode_pcm16_mono_wav(&[], 16_000);
        assert_eq!(WAV_HEADER_LEN, wav.len());
        assert_eq!(&36u32.to_le_bytes(), wav.get(4..8).unwrap());
        assert_eq!(&0u32.to_le_bytes(), wav.get(40..44).unwrap());
    }

    #[test]
    fn odd_trailing_byte_is_dropped_to_keep_frames_aligned() {
        let wav = encode_pcm16_mono_wav(&[1, 2, 3], 8_000);
        assert_eq!(WAV_HEADER_LEN + 2, wav.len());
        assert_eq!(&2u32.to_le_bytes(), wav.get(40..44).unwrap());
        assert_eq!(&[1, 2], wav.get(WAV_HEADER_LEN..).unwrap());
    }
}
