//! Turn the encoder sidecar's FLV output back into Annex-B access units.
//!
//! The sidecar used to write a raw Annex-B elementary stream, where a NAL
//! unit's end is only known once the *next* start code arrives. A frame could
//! not be finished until the next frame began (on a still desktop: the next
//! heartbeat, a second later), and splitting on start codes shipped SPS, PPS
//! and the picture slice as separate packets, which the phone's decoder cannot
//! use — it needs keyframes as whole access units with their parameter sets.
//!
//! FLV frames every access unit in a tag that states its own length, flags
//! keyframes, and carries the parameter sets in a sequence-header tag. So the
//! sidecar now writes FLV and this module converts each video tag into one
//! Annex-B access unit, prefixing keyframes with the parameter sets when the
//! frame does not already carry them. Handles legacy AVC tags (codec id 7) and
//! enhanced-FLV HEVC (`hvc1`), which is what FFmpeg 6.1+ writes for H.265.

/// One encoded frame, ready to ship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccessUnit {
    /// Annex-B: `00 00 00 01` + NAL, repeated.
    pub data: Vec<u8>,
    /// A random-access point (IDR / IRAP) the decoder can start from.
    pub keyframe: bool,
}

/// FLV header (9 bytes) plus the first PreviousTagSize (4 bytes).
const FILE_PREFIX: usize = 13;
/// TagType, DataSize, Timestamp(+ext), StreamID.
const TAG_HEADER: usize = 11;
/// PreviousTagSize after every tag.
const TAG_TRAILER: usize = 4;
const TAG_VIDEO: u8 = 9;
const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// Incremental FLV video demuxer.
#[derive(Debug, Default)]
pub(crate) struct FlvDemux {
    buf: Vec<u8>,
    header_skipped: bool,
    /// Bytes per NAL length prefix (from the decoder configuration record).
    nal_length_size: usize,
    /// Parameter-set NALs (Annex-B framed) from the latest sequence header.
    params: Vec<u8>,
    hevc: bool,
}

impl FlvDemux {
    pub(crate) fn new() -> Self {
        Self {
            nal_length_size: 4,
            ..Self::default()
        }
    }

    /// Append bytes read from the sidecar.
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete access unit, or `None` until more bytes arrive.
    pub(crate) fn next_access_unit(&mut self) -> Option<AccessUnit> {
        if !self.header_skipped {
            if self.buf.len() < FILE_PREFIX {
                return None;
            }
            // DataOffset (bytes 5..9) is the header length, 9 in every
            // version; honour it anyway.
            let offset =
                u32::from_be_bytes([self.buf[5], self.buf[6], self.buf[7], self.buf[8]]) as usize;
            let skip = offset + TAG_TRAILER;
            if self.buf.len() < skip {
                return None;
            }
            self.buf.drain(..skip);
            self.header_skipped = true;
        }
        loop {
            if self.buf.len() < TAG_HEADER {
                return None;
            }
            let tag_type = self.buf[0] & 0x1F;
            let size = u32::from_be_bytes([0, self.buf[1], self.buf[2], self.buf[3]]) as usize;
            let total = TAG_HEADER + size + TAG_TRAILER;
            if self.buf.len() < total {
                return None;
            }
            let body: Vec<u8> = self.buf[TAG_HEADER..TAG_HEADER + size].to_vec();
            self.buf.drain(..total);
            if tag_type == TAG_VIDEO
                && let Some(unit) = self.video_tag(&body)
            {
                return Some(unit);
            }
        }
    }

    /// Interpret one video tag body; `Some` for a coded frame.
    fn video_tag(&mut self, body: &[u8]) -> Option<AccessUnit> {
        let first = *body.first()?;
        let frame_type = (first >> 4) & 0x07;
        let keyframe = frame_type == 1;
        if first & 0x80 != 0 {
            // Enhanced FLV: [ex|frametype|packettype][fourcc x4]...
            let packet_type = first & 0x0F;
            if body.len() < 5 {
                return None;
            }
            self.hevc = &body[1..5] == b"hvc1";
            return match packet_type {
                0 => {
                    self.read_config(&body[5..]);
                    None
                }
                // CodedFrames carries a 3-byte composition time first.
                1 => self.frame(body.get(8..)?, keyframe),
                3 => self.frame(&body[5..], keyframe),
                _ => None,
            };
        }
        // Legacy: [frametype|codecid][packettype][composition time x3]...
        let codec_id = first & 0x0F;
        self.hevc = codec_id == 12;
        let packet_type = *body.get(1)?;
        let payload = body.get(5..)?;
        match packet_type {
            0 => {
                self.read_config(payload);
                None
            }
            1 => self.frame(payload, keyframe),
            _ => None,
        }
    }

    /// Store the parameter sets from an AVC/HEVC decoder configuration record.
    fn read_config(&mut self, record: &[u8]) {
        self.params.clear();
        if self.hevc {
            self.read_hevc_config(record);
        } else {
            self.read_avc_config(record);
        }
    }

    fn read_avc_config(&mut self, r: &[u8]) {
        let Some(&size_byte) = r.get(4) else { return };
        self.nal_length_size = usize::from(size_byte & 0x03) + 1;
        let mut pos = 5;
        // SPS list, then PPS list.
        for count_mask in [0x1F_u8, 0xFF] {
            let Some(&count) = r.get(pos) else { return };
            pos += 1;
            for _ in 0..(count & count_mask) {
                let Some(len) = r
                    .get(pos..pos + 2)
                    .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
                else {
                    return;
                };
                pos += 2;
                let Some(nal) = r.get(pos..pos + len) else {
                    return;
                };
                self.params.extend_from_slice(&START_CODE);
                self.params.extend_from_slice(nal);
                pos += len;
            }
        }
    }

    fn read_hevc_config(&mut self, r: &[u8]) {
        let Some(&size_byte) = r.get(21) else { return };
        self.nal_length_size = usize::from(size_byte & 0x03) + 1;
        let Some(&arrays) = r.get(22) else { return };
        let mut pos = 23;
        for _ in 0..arrays {
            let Some(count) = r
                .get(pos + 1..pos + 3)
                .map(|b| u16::from_be_bytes([b[0], b[1]]))
            else {
                return;
            };
            pos += 3;
            for _ in 0..count {
                let Some(len) = r
                    .get(pos..pos + 2)
                    .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
                else {
                    return;
                };
                pos += 2;
                let Some(nal) = r.get(pos..pos + len) else {
                    return;
                };
                self.params.extend_from_slice(&START_CODE);
                self.params.extend_from_slice(nal);
                pos += len;
            }
        }
    }

    fn is_parameter_set(&self, nal_header: u8) -> bool {
        if self.hevc {
            matches!((nal_header >> 1) & 0x3F, 32..=34) // VPS, SPS, PPS
        } else {
            matches!(nal_header & 0x1F, 7 | 8) // SPS, PPS
        }
    }

    /// Length-prefixed NALs → one Annex-B access unit.
    fn frame(&self, mut data: &[u8], keyframe: bool) -> Option<AccessUnit> {
        let n = self.nal_length_size;
        let mut out = Vec::with_capacity(data.len() + self.params.len() + 16);
        let mut has_params = false;
        while data.len() >= n {
            let len = data[..n]
                .iter()
                .fold(0usize, |acc, &b| (acc << 8) | usize::from(b));
            let nal = data.get(n..n + len)?;
            if let Some(&header) = nal.first() {
                has_params |= self.is_parameter_set(header);
                out.extend_from_slice(&START_CODE);
                out.extend_from_slice(nal);
            }
            data = &data[n + len..];
        }
        if out.is_empty() {
            return None;
        }
        if keyframe && !has_params && !self.params.is_empty() {
            let mut with_params = self.params.clone();
            with_params.extend_from_slice(&out);
            out = with_params;
        }
        Some(AccessUnit {
            data: out,
            keyframe,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_header() -> Vec<u8> {
        let mut v = b"FLV".to_vec();
        v.extend_from_slice(&[1, 0x01, 0, 0, 0, 9, 0, 0, 0, 0]);
        v
    }

    fn tag(tag_type: u8, body: &[u8]) -> Vec<u8> {
        let size = body.len() as u32;
        let mut v = vec![tag_type];
        v.extend_from_slice(&size.to_be_bytes()[1..]);
        v.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0]); // timestamp + ext + stream id
        v.extend_from_slice(body);
        v.extend_from_slice(&((TAG_HEADER as u32) + size).to_be_bytes());
        v
    }

    const SPS: &[u8] = &[0x67, 0x64, 0x00, 0x1F];
    const PPS: &[u8] = &[0x68, 0xEE, 0x3C];
    const IDR: &[u8] = &[0x65, 0x88, 0x84, 0x00, 0x10];
    const P: &[u8] = &[0x41, 0x9A, 0x02];

    fn avc_sequence_header() -> Vec<u8> {
        let mut body = vec![0x17, 0, 0, 0, 0];
        body.extend_from_slice(&[1, 0x64, 0, 0x1F, 0xFF, 0xE1]);
        body.extend_from_slice(&(SPS.len() as u16).to_be_bytes());
        body.extend_from_slice(SPS);
        body.push(1);
        body.extend_from_slice(&(PPS.len() as u16).to_be_bytes());
        body.extend_from_slice(PPS);
        body
    }

    fn avc_frame(keyframe: bool, nals: &[&[u8]]) -> Vec<u8> {
        let mut body = vec![if keyframe { 0x17 } else { 0x27 }, 1, 0, 0, 0];
        for nal in nals {
            body.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            body.extend_from_slice(nal);
        }
        body
    }

    fn annex_b(nals: &[&[u8]]) -> Vec<u8> {
        nals.iter()
            .flat_map(|n| [&START_CODE[..], n].concat())
            .collect()
    }

    fn stream() -> Vec<u8> {
        let mut s = file_header();
        s.extend(tag(18, b"onMetaData-ish")); // script tag: skipped
        s.extend(tag(TAG_VIDEO, &avc_sequence_header()));
        s.extend(tag(TAG_VIDEO, &avc_frame(true, &[IDR])));
        s.extend(tag(TAG_VIDEO, &avc_frame(false, &[P])));
        s
    }

    #[test]
    fn keyframes_carry_their_parameter_sets_and_each_tag_is_one_frame() {
        let mut demux = FlvDemux::new();
        demux.push(&stream());
        let key = demux.next_access_unit().expect("keyframe");
        assert!(key.keyframe);
        assert_eq!(key.data, annex_b(&[SPS, PPS, IDR]));
        let p = demux.next_access_unit().expect("P-frame");
        assert!(!p.keyframe);
        assert_eq!(p.data, annex_b(&[P]));
        assert_eq!(demux.next_access_unit(), None);
    }

    #[test]
    fn a_frame_is_released_the_moment_its_last_byte_arrives() {
        let bytes = stream();
        let mut demux = FlvDemux::new();
        let mut units = Vec::new();
        // One byte at a time: nothing is released early, nothing waits for
        // the next frame to begin.
        for (i, b) in bytes.iter().enumerate() {
            demux.push(&[*b]);
            while let Some(u) = demux.next_access_unit() {
                units.push((i, u));
            }
        }
        assert_eq!(units.len(), 2);
        assert_eq!(
            units[1].0,
            bytes.len() - 1,
            "the last frame needs no successor"
        );
    }

    #[test]
    fn in_band_parameter_sets_are_not_duplicated() {
        let mut s = file_header();
        s.extend(tag(TAG_VIDEO, &avc_sequence_header()));
        s.extend(tag(TAG_VIDEO, &avc_frame(true, &[SPS, PPS, IDR])));
        let mut demux = FlvDemux::new();
        demux.push(&s);
        assert_eq!(
            demux.next_access_unit().unwrap().data,
            annex_b(&[SPS, PPS, IDR])
        );
    }

    #[test]
    fn enhanced_flv_hevc_frames_get_vps_sps_pps() {
        const VPS: &[u8] = &[0x40, 0x01, 0x0C];
        const HSPS: &[u8] = &[0x42, 0x01, 0x01];
        const HPPS: &[u8] = &[0x44, 0x01, 0xC1];
        const IRAP: &[u8] = &[0x26, 0x01, 0xAF];
        let mut record = vec![1u8; 21];
        record.push(0xFF); // lengthSizeMinusOne = 3
        record.push(3); // three arrays
        for nal in [VPS, HSPS, HPPS] {
            record.push(nal[0] >> 1);
            record.extend_from_slice(&1u16.to_be_bytes());
            record.extend_from_slice(&(nal.len() as u16).to_be_bytes());
            record.extend_from_slice(nal);
        }
        let mut seq = vec![0x80 | 0x10]; // ex header, keyframe, SequenceStart
        seq.extend_from_slice(b"hvc1");
        seq.extend_from_slice(&record);
        let mut coded = vec![0x80 | 0x10 | 0x01]; // CodedFrames
        coded.extend_from_slice(b"hvc1");
        coded.extend_from_slice(&[0, 0, 0]); // composition time
        coded.extend_from_slice(&(IRAP.len() as u32).to_be_bytes());
        coded.extend_from_slice(IRAP);
        let mut s = file_header();
        s.extend(tag(TAG_VIDEO, &seq));
        s.extend(tag(TAG_VIDEO, &coded));
        let mut demux = FlvDemux::new();
        demux.push(&s);
        let unit = demux.next_access_unit().expect("HEVC keyframe");
        assert!(unit.keyframe);
        assert_eq!(unit.data, annex_b(&[VPS, HSPS, HPPS, IRAP]));
    }

    #[test]
    fn a_truncated_stream_waits_instead_of_guessing() {
        let bytes = stream();
        let mut demux = FlvDemux::new();
        demux.push(&bytes[..bytes.len() - 1]);
        assert!(
            demux.next_access_unit().is_some(),
            "the keyframe is complete"
        );
        assert_eq!(
            demux.next_access_unit(),
            None,
            "the P-frame is one byte short"
        );
    }
}
