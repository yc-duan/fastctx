//! A received body remains admitted while its consumer reads or installs it.
use crate::admission::BytePermit;

#[derive(Debug)]
pub struct Received<T> {
    value: T,
    _permit: BytePermit,
}
impl<T> Received<T> {
    pub fn new(value: T, permit: BytePermit) -> Self {
        Self { value, _permit: permit }
    }
    pub fn value(&self) -> &T {
        &self.value
    }
}
impl<T> std::ops::Deref for Received<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

mod json;
pub use json::{decode_json, decode_message};
