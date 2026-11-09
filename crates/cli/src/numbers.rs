//! Numbers as people write them on the command line: `5000`, `1_000_000`, `2m`, `1.5b`, `100t`, `1e12`.

const EXPECTED: &str = "expected a whole number such as 5000, 2m, 1.5b or 100t";

/// A count of deals, positions or the like. Worked out in 128 bits and in whole numbers throughout, so that `1.1t` is
/// exactly 1,100,000,000,000; anything that doesn't come to a whole number that fits is refused.
pub fn parse_count(text: &str) -> Result<u64, String> {
    count(text).ok_or_else(|| EXPECTED.to_string())
}

fn count(text: &str) -> Option<u64> {
    let (number, power) = split_power(text)?;
    let (whole, part) = match number.split_once('.') {
        Some((whole, part)) => (whole, Some(part)),
        None => (number, None),
    };
    let (mut digits, mut decimals) = (digits_of(whole)?, 0);
    if let Some(part) = part {
        let part = digits_of(part)?;
        decimals = part.len() as u32;
        digits.extend(part);
    }
    let value = digits.into_iter().try_fold(0u128, |v, d| v.checked_mul(10)?.checked_add(u128::from(d)))?;
    let value = if power >= decimals {
        value.checked_mul(10u128.checked_pow(power - decimals)?)?
    } else {
        let divisor = 10u128.checked_pow(decimals - power)?;
        if !value.is_multiple_of(divisor) {
            return None;
        }
        value / divisor
    };
    u64::try_from(value).ok()
}

/// The number before its suffix or exponent, and the power of ten that those stand for.
fn split_power(text: &str) -> Option<(&str, u32)> {
    if let Some((number, exponent)) = text.split_once(['e', 'E']) {
        let exponent = exponent.strip_prefix('+').unwrap_or(exponent);
        if exponent.is_empty() || !exponent.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        return Some((number, exponent.parse().ok()?));
    }
    let power = match text.bytes().last()?.to_ascii_lowercase() {
        b'k' => 3,
        b'm' => 6,
        b'b' => 9,
        b't' => 12,
        _ => return Some((text, 0)),
    };
    Some((&text[..text.len() - 1], power))
}

/// The digits of `text`, in which `_` may stand between digits.
fn digits_of(text: &str) -> Option<Vec<u8>> {
    let mut digits = Vec::with_capacity(text.len());
    for group in text.split('_') {
        if group.is_empty() || !group.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.extend(group.bytes().map(|b| b - b'0'));
    }
    Some(digits)
}

/// A memory size in GB from the command line has to fit in bytes.
pub fn check_gigabytes(flag: &str, gb: u64) -> Result<(), String> {
    if gb > u64::MAX >> 30 {
        return Err(format!("{flag} is too large: it can be at most {} (GB)", u64::MAX >> 30));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_numbers_and_underscores() {
        assert_eq!(parse_count("0"), Ok(0));
        assert_eq!(parse_count("5000"), Ok(5000));
        assert_eq!(parse_count("1_000_000"), Ok(1_000_000));
        assert_eq!(parse_count("18446744073709551615"), Ok(u64::MAX));
    }

    #[test]
    fn suffixes_in_either_case_with_decimals() {
        assert_eq!(parse_count("2k"), Ok(2_000));
        assert_eq!(parse_count("2K"), Ok(2_000));
        assert_eq!(parse_count("2m"), Ok(2_000_000));
        assert_eq!(parse_count("2M"), Ok(2_000_000));
        assert_eq!(parse_count("1.5b"), Ok(1_500_000_000));
        assert_eq!(parse_count("1.5B"), Ok(1_500_000_000));
        assert_eq!(parse_count("100t"), Ok(100_000_000_000_000));
        assert_eq!(parse_count("100T"), Ok(100_000_000_000_000));
        assert_eq!(parse_count("2.50k"), Ok(2_500));
        assert_eq!(parse_count("1_000.5k"), Ok(1_000_500));
        // Exact where a double would not be.
        assert_eq!(parse_count("1.1t"), Ok(1_100_000_000_000));
        assert_eq!(parse_count("9007.199254740993t"), Ok(9_007_199_254_740_993));
        assert_eq!(parse_count("18.446744073709551615e18"), Ok(u64::MAX));
    }

    #[test]
    fn exponents() {
        assert_eq!(parse_count("1e12"), Ok(1_000_000_000_000));
        assert_eq!(parse_count("1E12"), Ok(1_000_000_000_000));
        assert_eq!(parse_count("1e+3"), Ok(1_000));
        assert_eq!(parse_count("1.5e3"), Ok(1_500));
        assert_eq!(parse_count("2.0e0"), Ok(2));
        assert_eq!(parse_count("2.0"), Ok(2));
        assert_eq!(parse_count("1e19"), Ok(10_000_000_000_000_000_000));
    }

    #[test]
    fn what_is_refused_says_what_is_wanted() {
        for bad in [
            "",
            " ",
            "1,000",
            "1,5m",
            "1.5",
            "1.2345k",
            "1.5e0",
            "18446744073709551616",
            "1e20",
            "20000000t",
            "-5",
            "+5",
            "1__0",
            "_1",
            "1_",
            "k",
            "1kk",
            "1e",
            "e5",
            "1e-3",
            "1e3k",
            "1.",
            ".5m",
            "1.2.3",
            "1 000",
            "0x10",
            "1e99999999999",
            "ten",
            "١٢",
        ] {
            assert_eq!(parse_count(bad), Err(EXPECTED.to_string()), "{bad:?}");
        }
    }

    #[test]
    fn gigabytes_have_to_fit_in_bytes() {
        assert!(check_gigabytes("--memory", 16).is_ok());
        assert!(check_gigabytes("--memory", u64::MAX >> 30).is_ok());
        let error = check_gigabytes("--memory", (u64::MAX >> 30) + 1).unwrap_err();
        assert!(error.contains("--memory"), "{error}");
    }
}
