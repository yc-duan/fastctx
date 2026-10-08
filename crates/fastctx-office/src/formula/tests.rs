//! Frozen MS-XLS Ptg encodings and Apache POI NumberToTextConverter cases.
use super::*;

#[test]
fn biff8_reference_and_scalar_contract() {
    assert_eq!(
        crate::biff_string::read(&[&[4, 0, 0, b'A'], &[1, 0x3d, 0xd8], &[1, 0x00, 0xde, 0x2d, 0x4e]]).unwrap(),
        "A😀中"
    );
    let context = Context::default();
    let negative = [0x2c, 0xff, 0xff, 0xff, 0xff];
    assert_eq!(render(&negative, &[], (0, 0), true, &context).unwrap(), "IV65536");
    assert_eq!(render(&negative, &[], (1, 0), true, &context).unwrap(), "IV1");
    let positive = [0x2c, 1, 0, 1, 0xc0];
    assert_eq!(render(&positive, &[], (65535, 255), true, &context).unwrap(), "A1");
    assert_eq!(render(&[0x24, 0, 0, 0, 0x40], &[], (99, 99), false, &context).unwrap(), "A$1");
    assert_eq!(render(&[0x24, 0, 0, 0, 0x80], &[], (99, 99), false, &context).unwrap(), "$A1");
    for (value, expected) in [
        (1.2345678901234567, "1.23456789012346"),
        (1234567890123456.0, "1234567890123460"),
        (999999999999999.9, "1000000000000000"),
        (1e20, "1E+20"),
        (1.2345678901234567e-10, "1.23456789012346E-10"),
        (f64::MIN_POSITIVE, "2.2250738585072E-308"),
        (f64::MAX, "1.7976931348623E+308"),
        (-0.0, "-0"),
    ] {
        assert_eq!(number::text(value).unwrap(), expected);
    }
    assert_eq!(crate::number::format("46296", 46296.0, Some("yyyy-mm-dd"), false), "2026-10-01");
    assert_eq!(crate::number::format("0", 0.0, Some("yyyy-mm-dd"), true), "1904-01-01");
    assert_eq!(crate::number::format("60", 60.0, Some("yyyy-mm-dd"), false), "1900-02-29");
    assert_eq!(
        crate::number::format("45000.000001", 45000.000001, Some("yyyy-mm-dd hh:mm:ss.000"), false),
        "2023-03-15 00:00:00.086"
    );
    assert_eq!(crate::number::format("2.125", 2.125, Some("[hh]:mm:ss"), false), "2.125 (format \"[hh]:mm:ss\")");
    assert_eq!(crate::number::format("2", 2.0, Some("[<1]yyyy-mm-dd;0"), false), "2 (format \"[<1]yyyy-mm-dd;0\")");
    assert_eq!(crate::number::saved("46296", 46296.0, None, Some(31), false), "2026-10-01");
    assert_eq!(crate::number::saved("0.25", 0.25, None, Some(32), false), "06:00:00");
    let from = office_oxide::xlsx::CellRef::parse("B1").unwrap();
    let to = office_oxide::xlsx::CellRef::parse("C2").unwrap();
    assert_eq!(
        crate::xlsx_formula::translate("SUM(A : A)+SUM(1:1)+S1:S2!A1+LOG10(A1)+Table1[A1]", &from, &to).unwrap(),
        "SUM(B : B)+SUM(2:2)+S1:S2!B2+LOG10(B2)+Table1[A1]"
    );
}
