use anyhow::{Context, Result};

pub(crate) struct Bytes<'a>(pub &'a [u8]);

impl<'a> Bytes<'a> {
    pub fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let value = self.0.get(..count).context("truncated BIFF expression")?;
        self.0 = &self.0[count..];
        Ok(value)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into()?))
    }
    pub fn string(&mut self, count: usize) -> Result<String> {
        let wide = self.u8()?;
        anyhow::ensure!(wide <= 1, "unsupported BIFF string flags 0x{wide:02X}");
        let units: Vec<u16> = if wide == 0 {
            self.take(count)?.iter().map(|&b| u16::from(b)).collect()
        } else {
            self.take(count.checked_mul(2).context("BIFF string length overflow")?)?
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect()
        };
        String::from_utf16(&units).context("invalid UTF-16 in BIFF string")
    }
    pub fn long_string(&mut self) -> Result<String> {
        let count = self.u16()? as usize;
        self.string(count)
    }
}
