//! Locale-independent legacy text statistics, shared by viewing and writing.

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::Encoding;

use super::{TextEncoding, explicit};

pub(super) const BASIS: &str = "guessed from its bytes";

pub(super) struct Statistics(EncodingDetector);

impl Statistics {
    pub fn new() -> Self {
        Self(EncodingDetector::new(Iso2022JpDetection::Deny))
    }

    pub fn feed(&mut self, bytes: &[u8], last: bool) {
        self.0.feed(bytes, last);
    }

    pub fn score(&self, codec: TextEncoding) -> Option<i64> {
        let mut encoding = Encoding::for_label(codec.label().as_bytes())?;
        if encoding == encoding_rs::GB18030 {
            encoding = encoding_rs::GBK;
        }
        self.0.find_score(encoding)
    }

    pub fn ranked(&self) -> Vec<(TextEncoding, i64)> {
        // Only these models exist in chardetng 1.0.0. Its score API returns
        // zero for unknown models, and is not semver-stable: pin the dependency.
        let labels = [
            "gbk",
            "euc-jp",
            "euc-kr",
            "shift_jis",
            "big5",
            "windows-1252",
            "windows-1251",
            "windows-1250",
            "iso-8859-2",
            "windows-1256",
            "windows-1254",
            "windows-874",
            "windows-1255",
            "windows-1253",
            "iso-8859-7",
            "windows-1257",
            "iso-8859-13",
            "koi8-u",
            "ibm866",
            "iso-8859-6",
            "windows-1258",
            "iso-8859-4",
            "iso-8859-5",
            "iso-8859-8",
        ];
        let mut ranked: Vec<_> = labels
            .into_iter()
            .filter_map(|label| {
                let codec = explicit(label, &[]).expect("known detector model");
                self.score(codec).map(|score| (codec, score))
            })
            .collect();
        let native: Vec<_> = fastctx_platform::legacy_text::code_pages()
            .into_iter()
            .filter_map(|page| page.label().and_then(|label| explicit(label, &[]).ok()))
            .collect();
        ranked.sort_by(|(a, a_score), (b, b_score)| {
            b_score
                .cmp(a_score)
                .then_with(|| native.contains(b).cmp(&native.contains(a)))
                .then_with(|| a.label().cmp(b.label()))
        });
        ranked
    }

    pub fn leaders(&self) -> Vec<TextEncoding> {
        let guess = self.0.guess(None, Utf8Detection::Deny);
        let codec = explicit(guess.name(), &[]).expect("known detector result");
        let Some(score) = self.score(codec).filter(|score| *score > 0) else {
            return Vec::new();
        };
        // Zero is the detector's fallback baseline, not positive evidence.
        // Scores are evidence weights, not probabilities. Keep exact ties for
        // comparison of decoded text; never invent a confidence percentage.
        let ranked = self.ranked();
        if ranked.iter().any(|(_, value)| *value > score) {
            return Vec::new();
        }
        let mut leaders = vec![codec];
        leaders.extend(
            ranked.into_iter().filter_map(|(other, value)| (other != codec && value == score).then_some(other)),
        );
        leaders
    }
}
