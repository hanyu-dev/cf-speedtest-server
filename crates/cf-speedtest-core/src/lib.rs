//! Generate zstd compressed zeros.

use std::num::NonZeroU64;

/// `Content-Encoding` header value
pub const CONTENT_ENCODING: &str = "zstd";

/// Server version
pub static VERSION: &str = include_str!(concat!(env!("OUT_DIR"), "/VERSION"));

/// Generate a minimal `zstd` compressed bytes array that decompresses to
/// `target` bytes of zeros.
pub fn zeros(target: NonZeroU64) -> Vec<u8> {
    let mut output = Vec::with_capacity(4096);

    // Magic number.
    output.extend(&[0x28, 0xB5, 0x2F, 0xFD]);

    // Frame Header Descriptor
    //
    // 7-6: Frame Content Size Flag, 0b00=u8, 0b01=u16, 0b10=u32, 0b11=u64
    // 5: Single Segment Flag, we set to 0
    // 4: (unused)
    // 3: (reserved)
    // 2: Checksum, we set to 0.
    // 1-0: Dictionary ID Flag, we set to 0b00.
    const FCS_FLAG_0: u8 = 0b_00_0_0_0_0_00;
    const FCS_FLAG_1: u8 = 0b_01_0_0_0_0_00;
    const FCS_FLAG_2: u8 = 0b_10_0_0_0_0_00;
    const FCS_FLAG_3: u8 = 0b_11_0_0_0_0_00;

    let fcs_flag = {
        const FCS_SIZE_FLAG_0_MIN: u64 = 0;
        const FCS_SIZE_FLAG_0_MAX: u64 = u8::MAX as u64;
        const FCS_SIZE_FLAG_1_MIN: u64 = 256;
        const FCS_SIZE_FLAG_1_MAX: u64 = u16::MAX as u64 + 256;
        const FCS_SIZE_FLAG_2_MIN: u64 = 0;
        const FCS_SIZE_FLAG_2_MAX: u64 = u32::MAX as u64;
        const FCS_SIZE_FLAG_3_MIN: u64 = 0;
        const FCS_SIZE_FLAG_3_MAX: u64 = u64::MAX;

        let fcs_flag = match target.get() {
            0 => FCS_FLAG_0,
            FCS_SIZE_FLAG_0_MIN..=FCS_SIZE_FLAG_0_MAX => FCS_FLAG_0,
            FCS_SIZE_FLAG_1_MIN..=FCS_SIZE_FLAG_1_MAX => FCS_FLAG_1,
            FCS_SIZE_FLAG_2_MIN..=FCS_SIZE_FLAG_2_MAX => FCS_FLAG_2,
            FCS_SIZE_FLAG_3_MIN..=FCS_SIZE_FLAG_3_MAX => FCS_FLAG_3,
        };

        output.push(fcs_flag);

        fcs_flag
    };

    // Window Descriptor
    //
    // 7-3: Exponent.
    // 2-0: Mantissa (we set to 0b000 for simplicity).
    //
    // For improved interoperability, we limit the window size to
    // 16 MiB (2^24).
    let window_size = {
        #[inline(always)]
        const fn window_size(exponent: u8) -> u64 {
            1 << (10 + exponent)
        }

        const WINDOW_EXPONENT_0: u8 = 0b00000;
        const WINDOW_EXPONENT_1: u8 = 0b00001;
        const WINDOW_EXPONENT_2: u8 = 0b00010;
        const WINDOW_EXPONENT_3: u8 = 0b00011;
        const WINDOW_EXPONENT_4: u8 = 0b00100;
        const WINDOW_EXPONENT_5: u8 = 0b00101;
        const WINDOW_EXPONENT_6: u8 = 0b00110;
        const WINDOW_EXPONENT_7: u8 = 0b00111;
        const WINDOW_EXPONENT_8: u8 = 0b01000;
        const WINDOW_EXPONENT_9: u8 = 0b01001;
        const WINDOW_EXPONENT_10: u8 = 0b01010;
        const WINDOW_EXPONENT_11: u8 = 0b01011;
        const WINDOW_EXPONENT_12: u8 = 0b01100;
        const WINDOW_EXPONENT_13: u8 = 0b01101;
        const WINDOW_EXPONENT_14: u8 = 0b01110;

        let window_exponent = match target.get() {
            target if target > const { window_size(WINDOW_EXPONENT_14) } => WINDOW_EXPONENT_14,
            target if target > const { window_size(WINDOW_EXPONENT_13) } => WINDOW_EXPONENT_13,
            target if target > const { window_size(WINDOW_EXPONENT_12) } => WINDOW_EXPONENT_12,
            target if target > const { window_size(WINDOW_EXPONENT_11) } => WINDOW_EXPONENT_11,
            target if target > const { window_size(WINDOW_EXPONENT_10) } => WINDOW_EXPONENT_10,
            target if target > const { window_size(WINDOW_EXPONENT_9) } => WINDOW_EXPONENT_9,
            target if target > const { window_size(WINDOW_EXPONENT_8) } => WINDOW_EXPONENT_8,
            target if target > const { window_size(WINDOW_EXPONENT_7) } => WINDOW_EXPONENT_7,
            target if target > const { window_size(WINDOW_EXPONENT_6) } => WINDOW_EXPONENT_6,
            target if target > const { window_size(WINDOW_EXPONENT_5) } => WINDOW_EXPONENT_5,
            target if target > const { window_size(WINDOW_EXPONENT_4) } => WINDOW_EXPONENT_4,
            target if target > const { window_size(WINDOW_EXPONENT_3) } => WINDOW_EXPONENT_3,
            target if target > const { window_size(WINDOW_EXPONENT_2) } => WINDOW_EXPONENT_2,
            target if target > const { window_size(WINDOW_EXPONENT_1) } => WINDOW_EXPONENT_1,
            _ => WINDOW_EXPONENT_0,
        };

        output.push(window_exponent << 3);

        window_size(window_exponent)
    };

    // 3.1.1.1.4. Frame Content Size.
    match fcs_flag {
        FCS_FLAG_0 => {
            output.push(target.get() as u8);
        }
        FCS_FLAG_1 => {
            output.extend(&((target.get() - 256) as u16).to_le_bytes());
        }
        FCS_FLAG_2 => {
            output.extend(&(target.get() as u32).to_le_bytes());
        }
        FCS_FLAG_3 => {
            output.extend(&target.get().to_le_bytes());
        }
        _ => unreachable!(),
    }

    // Use RLE Literals Block
    const BLOCK_TYPE_RLE: u32 = 1;

    // Block maximum size is 128 KiB.
    const BLOCK_MAX_SIZE: u64 = 128 * 1024;

    let mut remaining = target.get();

    while remaining > 0 {
        // Determine block size.
        let block_size = remaining.min(window_size).min(BLOCK_MAX_SIZE);

        // Update remaining size
        remaining -= block_size;

        // Last Block flag
        let last_block = if remaining == 0 { 1 } else { 0 };

        // Block Header
        //
        // 0: Last Block
        // 1..=2: Block Type (0=Raw, 1=RLE, 2=Compressed, 3=Reserved)
        // 3..=23: Block Size (in bytes)
        //
        // Followed by:
        //
        // RLE byte (0x00)
        let block = (last_block | (BLOCK_TYPE_RLE << 1) | ((block_size as u32) << 3)).to_le_bytes();

        // RLE byte must be zero, since the window size will never exceed 128 KiB and
        // can be covered by u21.
        debug_assert!(block[3] == 0x00);

        output.extend(&block);
    }

    output
}
