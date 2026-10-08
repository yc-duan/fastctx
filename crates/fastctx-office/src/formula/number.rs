//! Excel formula literals use 15 significant decimal digits, then round
//! the last digit again for exponents outside -98..=98. Compute the exact
//! binary rational before rounding; a shortest float string can cross a tie.
use anyhow::{Result, ensure};
use num_bigint::BigUint;

pub(super) fn text(value: f64) -> Result<String> {
    ensure!(value.is_finite(), "non-finite formula number");
    let bits = value.to_bits();
    let sign = if value.is_sign_negative() { "-" } else { "" };
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    if exponent == 0 {
        return Ok(format!("{sign}0"));
    }
    let mantissa = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    let binary = exponent - 1023 - 52;
    let (integer, scale) = if binary >= 0 {
        (BigUint::from(mantissa) << binary as usize, 0)
    } else {
        (BigUint::from(mantissa) * BigUint::from(5u8).pow((-binary) as u32), -binary)
    };
    let mut digits = integer.to_str_radix(10);
    let mut exponent = digits.len() as i32 - scale - 1;
    round(&mut digits, 15, &mut exponent);
    if exponent.abs() > 98 {
        round(&mut digits, 14, &mut exponent);
    }
    while digits.ends_with('0') {
        digits.pop();
    }
    let scientific = exponent > 19 || (exponent < 0 && 1 - exponent + digits.len() as i32 > 20);
    let body = if scientific {
        let fraction = if digits.len() == 1 { String::new() } else { format!(".{}", &digits[1..]) };
        format!("{}{fraction}E{}{:02}", &digits[..1], if exponent < 0 { "-" } else { "+" }, exponent.abs())
    } else if exponent < 0 {
        format!("0.{}{}", "0".repeat((-exponent - 1) as usize), digits)
    } else {
        let integer = exponent as usize + 1;
        if integer >= digits.len() {
            format!("{}{}", digits, "0".repeat(integer - digits.len()))
        } else {
            format!("{}.{}", &digits[..integer], &digits[integer..])
        }
    };
    Ok(format!("{sign}{body}"))
}

fn round(digits: &mut String, count: usize, exponent: &mut i32) {
    if digits.len() <= count {
        return;
    }
    let up = digits.as_bytes()[count] >= b'5';
    digits.truncate(count);
    if up {
        let value = digits.parse::<u64>().expect("at most 15 decimal digits") + 1;
        *digits = value.to_string();
        if digits.len() > count {
            *exponent += 1;
            digits.truncate(count);
        }
    }
}
