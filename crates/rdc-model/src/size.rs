//! Human-readable byte sizes.

/// Units of successive powers of 1024.
const UNITS: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];

/// Formats a byte count with 1024-based units and at most two decimals,
/// e.g. `0 B`, `1023 B`, `1.5 KB`, `1.13 KB` (1152 bytes), `8 EB`.
///
/// `None` formats as `0 B`. The value is first reduced to 15 significant
/// digits, then rounded half away from zero; trailing zeros are dropped.
pub fn format_size(bytes: Option<i64>) -> String {
    let Some(bytes) = bytes else {
        return "0 B".to_owned();
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "the size is displayed with at most 15 significant digits"
    )]
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{} {}", two_decimals(size), UNITS[unit])
}

/// Formats `value` rounded to at most two decimals without trailing zeros.
fn two_decimals(value: f64) -> String {
    // 15 significant digits: `d.dddddddddddddde<exponent>`.
    let scientific = format!("{:.14e}", value.abs());
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("exponent notation always contains an exponent");
    let exponent: i32 = exponent.parse().expect("exponent is an integer");
    let digits: u128 = mantissa
        .replace('.', "")
        .parse()
        .expect("mantissa is made of digits");
    // value = digits * 10^(exponent - 14), so hundredths = digits * 10^(exponent - 12).
    let shift = exponent - 12;
    let hundredths = match u32::try_from(shift) {
        Ok(up) => digits * 10_u128.pow(up),
        Err(_) => 10_u128
            .checked_pow(shift.unsigned_abs())
            .map_or(0, |divisor| (digits + divisor / 2) / divisor),
    };
    let sign = if value < 0.0 && hundredths != 0 {
        "-"
    } else {
        ""
    };
    let (whole, fraction) = (hundredths / 100, hundredths % 100);
    if fraction == 0 {
        format!("{sign}{whole}")
    } else if fraction % 10 == 0 {
        format!("{sign}{whole}.{}", fraction / 10)
    } else {
        format!("{sign}{whole}.{fraction:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_size_is_zero_bytes() {
        assert_eq!(format_size(None), "0 B");
    }

    #[test]
    fn formats_each_unit() {
        let cases = [
            (0, "0 B"),
            (1, "1 B"),
            (1023, "1023 B"),
            (1024, "1 KB"),
            (1536, "1.5 KB"),
            (1024 * 1024, "1 MB"),
            (5 * 1024 * 1024 * 1024, "5 GB"),
            (1024_i64.pow(4), "1 TB"),
            (1024_i64.pow(5), "1 PB"),
            (1024_i64.pow(6), "1 EB"),
            (i64::MAX, "8 EB"),
        ];
        for (bytes, expected) in cases {
            assert_eq!(format_size(Some(bytes)), expected, "bytes {bytes}");
        }
    }

    #[test]
    fn rounds_to_two_decimals_half_away_from_zero() {
        let cases = [
            (1152, "1.13 KB"),
            (1126, "1.1 KB"),
            (1100, "1.07 KB"),
            (2047, "2 KB"),
            (1_572_864_000, "1.46 GB"),
            (1_048_575, "1024 KB"),
        ];
        for (bytes, expected) in cases {
            assert_eq!(format_size(Some(bytes)), expected, "bytes {bytes}");
        }
    }

    #[test]
    fn negative_sizes_stay_in_bytes() {
        assert_eq!(format_size(Some(-5)), "-5 B");
        assert_eq!(format_size(Some(-4096)), "-4096 B");
    }
}
