//! Fixed records spill before selection metadata can exceed the answer budget.
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

pub(super) struct Queue<const N: usize> {
    memory: VecDeque<[u64; N]>,
    file: Option<File>,
    first: u64,
    end: u64,
}
impl<const N: usize> Default for Queue<N> {
    fn default() -> Self {
        Self { memory: VecDeque::new(), file: None, first: 0, end: 0 }
    }
}
impl<const N: usize> Queue<N> {
    pub fn len(&self) -> usize {
        if self.file.is_some() { ((self.end - self.first) / (N * 8) as u64) as usize } else { self.memory.len() }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn push(&mut self, value: [u64; N]) -> io::Result<()> {
        if self.file.is_none() && self.memory.len() < crate::spool::MEMORY / (N * 8) {
            self.memory.push_back(value);
            return Ok(());
        }
        if self.file.is_none() {
            self.file = Some(tempfile::tempfile()?);
            while let Some(value) = self.memory.pop_front() {
                self.append(value)?;
            }
            self.memory = VecDeque::new();
        }
        self.append(value)
    }
    fn append(&mut self, value: [u64; N]) -> io::Result<()> {
        let file = self.file.as_mut().expect("spill");
        file.seek(SeekFrom::Start(self.end))?;
        for word in value {
            file.write_all(&word.to_le_bytes())?;
        }
        self.end += (N * 8) as u64;
        Ok(())
    }
    pub fn front(&mut self) -> io::Result<Option<[u64; N]>> {
        let Some(file) = self.file.as_mut() else {
            return Ok(self.memory.front().copied());
        };
        if self.first == self.end {
            return Ok(None);
        }
        file.seek(SeekFrom::Start(self.first))?;
        let mut value = [0; N];
        for word in &mut value {
            let mut bytes = [0; 8];
            file.read_exact(&mut bytes)?;
            *word = u64::from_le_bytes(bytes);
        }
        Ok(Some(value))
    }
    pub fn pop(&mut self) -> io::Result<Option<[u64; N]>> {
        if self.file.is_none() {
            return Ok(self.memory.pop_front());
        }
        let value = self.front()?;
        if value.is_some() {
            self.first += (N * 8) as u64;
        }
        Ok(value)
    }
}
