//! The picture size an H.264 stream's sequence parameter set (SPS) says
//! (BK7): a session's frames carry no size of their own, so another
//! computer's screen in a window is sized from its stream. Reads only as far
//! as the size and its cropping; anything malformed is `None`.

/// The picture's width and height, in pixels, from the first SPS in an
/// Annex-B access unit; `None` when it holds none, or one that does not read.
pub fn picture_size(access_unit: &[u8]) -> Option<(u16, u16)> {
    let sps = nal_units(access_unit).find(|nal| nal.first().is_some_and(|h| h & 0x1f == 7))?;
    sps_size(&unescape(&sps[1..]))
}

/// The NAL units of an Annex-B stream, each without its start code.
fn nal_units(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let ends: Vec<usize> = starts
        .iter()
        .skip(1)
        .map(|next| {
            // The next start code, and the zero of a four-byte one before it.
            let mut end = next - 3;
            while end > 0 && data[end - 1] == 0 {
                end -= 1;
            }
            end
        })
        .chain(std::iter::once(data.len()))
        .collect();
    starts
        .into_iter()
        .zip(ends)
        .filter(|(start, end)| start < end)
        .map(move |(start, end)| &data[start..end])
}

/// The NAL payload without its emulation prevention bytes (`00 00 03`).
fn unescape(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut zeros = 0;
    for &byte in payload {
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        zeros = if byte == 0 { zeros + 1 } else { 0 };
        out.push(byte);
    }
    out
}

/// Reads bits, most significant first, and exp-Golomb codes.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.at / 8)?;
        let bit = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Some(u32::from(bit))
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        (0..n).try_fold(0u32, |acc, _| Some((acc << 1) | self.bit()?))
    }

    /// An unsigned exp-Golomb code, `ue(v)`.
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let rest = self.bits(zeros)?;
        Some(((1u64 << zeros) - 1 + u64::from(rest)).min(u64::from(u32::MAX)) as u32)
    }

    /// A signed exp-Golomb code, `se(v)`.
    fn se(&mut self) -> Option<i32> {
        let k = self.ue()?;
        let magnitude = k.div_ceil(2) as i32;
        Some(if k % 2 == 1 { magnitude } else { -magnitude })
    }
}

/// The most macroblocks a frame has at H.264's largest level (6.2's MaxFS).
pub const MAX_FRAME_MBS: u64 = 139_264;
/// The longest side read, in pixels: 8K's width. A side past it is no
/// screen this window shows.
pub const MAX_SIDE: u64 = 8192;

/// The profiles whose SPS says its chroma format and bit depths.
const HIGH_PROFILES: [u32; 13] = [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

fn sps_size(rbsp: &[u8]) -> Option<(u16, u16)> {
    let mut r = Bits { data: rbsp, at: 0 };
    let profile = r.bits(8)?;
    r.bits(16)?; // constraint flags, level
    r.ue()?; // seq_parameter_set_id
    let mut chroma_format = 1;
    let mut separate_planes = false;
    if HIGH_PROFILES.contains(&profile) {
        chroma_format = r.ue()?;
        if chroma_format > 3 {
            return None;
        }
        if chroma_format == 3 {
            separate_planes = r.bit()? == 1;
        }
        r.ue()?; // bit_depth_luma_minus8
        r.ue()?; // bit_depth_chroma_minus8
        r.bit()?; // qpprime_y_zero_transform_bypass
        if r.bit()? == 1 {
            let lists = if chroma_format == 3 { 12 } else { 8 };
            for i in 0..lists {
                if r.bit()? == 1 {
                    skip_scaling_list(&mut r, if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    r.ue()?; // log2_max_frame_num_minus4
    match r.ue()? {
        0 => {
            r.ue()?; // log2_max_pic_order_cnt_lsb_minus4
        }
        1 => {
            r.bit()?; // delta_pic_order_always_zero
            r.se()?; // offset_for_non_ref_pic
            r.se()?; // offset_for_top_to_bottom_field
            let cycle = r.ue()?;
            if cycle > 255 {
                return None;
            }
            for _ in 0..cycle {
                r.se()?;
            }
        }
        2 => {}
        _ => return None,
    }
    r.ue()?; // max_num_ref_frames
    r.bit()?; // gaps_in_frame_num_value_allowed
    let width_mbs = u64::from(r.ue()?) + 1;
    let height_units = u64::from(r.ue()?) + 1;
    let frame_mbs_only = r.bit()? == 1;
    if !frame_mbs_only {
        r.bit()?; // mb_adaptive_frame_field
    }
    r.bit()?; // direct_8x8_inference
    let (mut left, mut right, mut top, mut bottom) = (0, 0, 0, 0);
    if r.bit()? == 1 {
        left = u64::from(r.ue()?);
        right = u64::from(r.ue()?);
        top = u64::from(r.ue()?);
        bottom = u64::from(r.ue()?);
    }
    let fields = if frame_mbs_only { 1 } else { 2 };
    let chroma_array = if separate_planes { 0 } else { chroma_format };
    let (crop_x, crop_y) = match chroma_array {
        0 => (1, fields),
        1 => (2, 2 * fields),
        2 => (2, fields),
        _ => (1, fields),
    };
    // No level allows more (BK7 review I1): a host's bogus size must not
    // make the window ask for gigabytes.
    // Checked (BK7 re-review N2): both counts reach 2^32.
    let frame_mbs = width_mbs.checked_mul(height_units)?.checked_mul(fields)?;
    if frame_mbs > MAX_FRAME_MBS {
        return None;
    }
    let width = (width_mbs * 16).checked_sub(crop_x * (left + right))?;
    let height = (height_units * 16 * fields).checked_sub(crop_y * (top + bottom))?;
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    Some((u16::try_from(width).ok()?, u16::try_from(height).ok()?))
}

fn skip_scaling_list(r: &mut Bits<'_>, size: usize) -> Option<()> {
    let (mut last, mut next) = (8i32, 8i32);
    for _ in 0..size {
        if next != 0 {
            // delta_scale is -128 to 127 (BK7 review M2: a wilder one
            // overflowed).
            let delta = r.se()?;
            if !(-128..=127).contains(&delta) {
                return None;
            }
            next = (last + delta + 256).rem_euclid(256);
        }
        if next != 0 {
            last = next;
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    /// An access unit as an encoder sends one: a start code, the SPS, and a
    /// slice after it.
    fn unit(sps: &str) -> Vec<u8> {
        let mut out = vec![0, 0, 0, 1];
        out.extend(hex(sps));
        out.extend([0, 0, 0, 1, 0x65, 0x88, 0x84, 0x00]);
        out
    }

    /// SPSes ffmpeg's libx264 wrote (High, and Baseline): sizes that are
    /// whole macroblocks, and sizes cropped from them.
    #[test]
    fn the_size_is_read_from_real_sps_bytes() {
        for (sps, size) in [
            (
                "67640028acd940780227e5c044000003000400000300503c60c658",
                (1920, 1080),
            ),
            (
                "67640020acd94056061e6f011000000300100000030140f1831960",
                (1366, 768),
            ),
            (
                "6764000aacd94411b011000003000100000300140f122596",
                (64, 128),
            ),
            (
                "6742c016d900a02ff97011000003000100000300140f162e48",
                (640, 360),
            ),
        ] {
            assert_eq!(picture_size(&unit(sps)), Some(size), "{sps}");
        }
    }

    #[test]
    fn a_unit_without_an_sps_or_with_a_broken_one_has_no_size() {
        assert_eq!(picture_size(&[0, 0, 0, 1, 0x65, 0x88, 0x84]), None);
        assert_eq!(picture_size(&[]), None);
        assert_eq!(picture_size(&[0, 0, 1, 0x67, 0x64]), None, "cut short");
        // Every prefix of a real one is short, never a wrong size.
        let whole = unit("67640028acd940780227e5c044000003000400000300503c60c658");
        for end in 0..12 {
            assert_eq!(picture_size(&whole[..end]), None, "{end} bytes");
        }
    }

    /// Writes bits and exp-Golomb codes, for SPSes made here.
    #[derive(Default)]
    struct Writer {
        bits: Vec<bool>,
    }

    impl Writer {
        fn bits(&mut self, value: u32, n: u32) -> &mut Self {
            for i in (0..n).rev() {
                self.bits.push((value >> i) & 1 == 1);
            }
            self
        }

        fn ue(&mut self, value: u32) -> &mut Self {
            let coded = u64::from(value) + 1;
            let len = 64 - coded.leading_zeros();
            for _ in 1..len {
                self.bits.push(false);
            }
            for i in (0..len).rev() {
                self.bits.push((coded >> i) & 1 == 1);
            }
            self
        }

        /// An access unit holding this SPS, its bits padded with the stop bit.
        fn unit(&mut self) -> Vec<u8> {
            self.bits.push(true);
            while !self.bits.len().is_multiple_of(8) {
                self.bits.push(false);
            }
            let mut out = vec![0, 0, 0, 1, 0x67];
            // With emulation prevention, as an encoder writes it: a run of
            // zero bits would otherwise read as the next start code.
            let mut zeros = 0;
            for byte in self
                .bits
                .chunks(8)
                .map(|byte| byte.iter().fold(0u8, |acc, b| (acc << 1) | u8::from(*b)))
            {
                if zeros >= 2 && byte <= 3 {
                    out.push(3);
                    zeros = 0;
                }
                zeros = if byte == 0 { zeros + 1 } else { 0 };
                out.push(byte);
            }
            out
        }
    }

    /// A Baseline SPS of `width_mbs` by `height_mbs` macroblocks, uncropped.
    fn baseline(width_mbs: u32, height_mbs: u32) -> Vec<u8> {
        Writer::default()
            .bits(66, 8)
            .bits(0, 8)
            .bits(62, 8)
            .ue(0) // seq_parameter_set_id
            .ue(0) // log2_max_frame_num_minus4
            .ue(2) // pic_order_cnt_type
            .ue(1) // max_num_ref_frames
            .bits(0, 1) // gaps
            .ue(width_mbs - 1)
            .ue(height_mbs - 1)
            .bits(1, 1) // frame_mbs_only
            .bits(1, 1) // direct_8x8_inference
            .bits(0, 1) // no cropping
            .unit()
    }

    /// BK7 review I1: a size no H.264 level allows (one a host could send
    /// to make the window ask for gigabytes) is no size; the largest Level
    /// 6.2 allows is.
    #[test]
    fn a_picture_larger_than_any_level_allows_has_no_size() {
        assert_eq!(picture_size(&baseline(40, 23)), Some((640, 368)));
        assert_eq!(
            picture_size(&baseline(512, 270)),
            Some((8192, 4320)),
            "8K, within Level 6.2"
        );
        assert_eq!(picture_size(&baseline(4096, 4096)), None, "65536 x 65536");
        assert_eq!(picture_size(&baseline(1024, 16)), None, "16384 wide");
        assert_eq!(picture_size(&baseline(16, 1024)), None, "16384 high");
        assert_eq!(
            picture_size(&baseline(500, 500)),
            None,
            "within each side, past the level's frame size"
        );
    }

    /// BK7 re-review N2: macroblock counts whose product passes u64 are no
    /// size, never an overflow.
    #[test]
    fn macroblock_counts_past_u64_are_no_size() {
        let sps = Writer::default()
            .bits(66, 8)
            .bits(0, 8)
            .bits(62, 8)
            .ue(0) // seq_parameter_set_id
            .ue(0) // log2_max_frame_num_minus4
            .ue(2) // pic_order_cnt_type
            .ue(1) // max_num_ref_frames
            .bits(0, 1) // gaps
            .ue(u32::MAX - 1) // width: 2^32 - 1 macroblocks
            .ue(u32::MAX - 1) // height: 2^32 - 1 map units
            .bits(0, 1) // fields: frame_mbs_only off
            .bits(0, 1) // mb_adaptive_frame_field
            .bits(1, 1) // direct_8x8_inference
            .bits(0, 1) // no cropping
            .unit();
        assert_eq!(picture_size(&sps), None);
    }

    /// BK7 review M2: a scaling list's step past what the standard allows
    /// (-128 to 127) makes the SPS unreadable, never an overflow.
    #[test]
    fn a_scaling_step_out_of_range_is_no_size() {
        let sps = |step_code: u32| {
            Writer::default()
                .bits(100, 8) // High
                .bits(0, 8)
                .bits(40, 8)
                .ue(0) // seq_parameter_set_id
                .ue(1) // chroma_format_idc
                .ue(0) // bit_depth_luma_minus8
                .ue(0) // bit_depth_chroma_minus8
                .bits(0, 1) // qpprime
                .bits(1, 1) // seq_scaling_matrix_present
                .bits(1, 1) // list 0 present
                .ue(step_code) // its first delta, se as ue
                .unit()
        };
        // se 2^31 - 1, coded as ue 2^32 - 3.
        assert_eq!(picture_size(&sps(4_294_967_293)), None);
        // se 128, one past the range.
        assert_eq!(picture_size(&sps(255)), None);
    }

    #[test]
    fn exp_golomb_codes_read_as_the_standard_says() {
        // 1 | 010 | 011 | 00100: ue 0, 1, 2, 3; se 4 is -2, se 1 is 1.
        let data = [0b1010_0110, 0b0100_0010, 0b1000_0000];
        let mut r = Bits { data: &data, at: 0 };
        assert_eq!(r.ue(), Some(0));
        assert_eq!(r.ue(), Some(1));
        assert_eq!(r.ue(), Some(2));
        assert_eq!(r.ue(), Some(3));
        let data = [0b0010_1000];
        assert_eq!(Bits { data: &data, at: 0 }.se(), Some(-2));
        let data = [0b0100_0000];
        assert_eq!(Bits { data: &data, at: 0 }.se(), Some(1));
    }

    #[test]
    fn emulation_prevention_bytes_are_taken_out() {
        assert_eq!(unescape(&[0, 0, 3, 1, 0, 0, 3, 0]), vec![0, 0, 1, 0, 0, 0]);
        assert_eq!(unescape(&[0, 3, 0]), vec![0, 3, 0], "one zero is not a run");
    }
}
