//! Sparse terminal cells. Large rows use a radix tree in an anonymous file.
//! The tree has sixteen fixed levels, independent of the number of cells.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

use crate::spool::MEMORY;

const PAGE: usize = 4096;
const CACHE: usize = MEMORY / PAGE;
const CELLS: usize = MEMORY / std::mem::size_of::<(u128, Cell)>();

#[derive(Clone, Copy, Default)]
pub(super) struct Cell {
    pub view: u64,
    head: u64,
    tail: u64,
    inline: u64,
}

impl Cell {
    pub fn new(text: &str, view: u64) -> Self {
        assert!(text.len() <= 4);
        let mut bytes = [0; 8];
        bytes[0] = text.len() as u8 + 1;
        bytes[1..1 + text.len()].copy_from_slice(text.as_bytes());
        Self { view, inline: u64::from_le_bytes(bytes), ..Self::default() }
    }
    pub fn empty(self) -> bool {
        self.inline == 1 && self.head == 0
    }
    pub fn tab(self) -> bool {
        self.head == 0 && self.inline == u64::from_le_bytes([2, b'\t', 0, 0, 0, 0, 0, 0])
    }
    fn exists(self) -> bool {
        self.inline != 0 || self.head != 0
    }
    fn bytes(self) -> [u8; 32] {
        let mut bytes = [0; 32];
        for (out, value) in bytes.chunks_exact_mut(8).zip([self.view, self.head, self.tail, self.inline]) {
            out.copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }
    fn of(bytes: [u8; 32]) -> Self {
        let mut values = bytes.chunks_exact(8).map(|bytes| u64::from_le_bytes(bytes.try_into().expect("word")));
        Self {
            view: values.next().unwrap(),
            head: values.next().unwrap(),
            tail: values.next().unwrap(),
            inline: values.next().unwrap(),
        }
    }
}

struct Page {
    bytes: Box<[u8; PAGE]>,
    dirty: bool,
    used: u64,
}

#[derive(Default)]
struct Disk {
    file: Option<File>,
    pages: BTreeMap<u64, Page>,
    end: u64,
    clock: u64,
}

impl Disk {
    fn allocate(&mut self, length: u64) -> io::Result<u64> {
        let at = self.end.max(PAGE as u64);
        self.end =
            at.checked_add(length).ok_or_else(|| io::Error::other("terminal working file exceeds addressable size"))?;
        Ok(at)
    }
    fn page(&mut self, id: u64) -> io::Result<&mut Page> {
        self.clock = self.clock.wrapping_add(1);
        if !self.pages.contains_key(&id) {
            if self.pages.len() == CACHE {
                let oldest = *self.pages.iter().min_by_key(|(_, p)| p.used).expect("cache").0;
                let page = self.pages.remove(&oldest).expect("cached");
                if page.dirty {
                    if self.file.is_none() {
                        self.file = Some(tempfile::tempfile()?);
                    }
                    let file = self.file.as_mut().expect("created");
                    file.seek(SeekFrom::Start(oldest * PAGE as u64))?;
                    file.write_all(page.bytes.as_slice())?;
                }
            }
            let mut bytes = Box::new([0; PAGE]);
            if let Some(file) = self.file.as_mut() {
                file.seek(SeekFrom::Start(id * PAGE as u64))?;
                let mut at = 0;
                while at < PAGE {
                    let read = file.read(&mut bytes[at..])?;
                    if read == 0 {
                        break;
                    }
                    at += read;
                }
            }
            self.pages.insert(id, Page { bytes, dirty: false, used: self.clock });
        }
        let page = self.pages.get_mut(&id).expect("inserted");
        page.used = self.clock;
        Ok(page)
    }
    fn read<const N: usize>(&mut self, at: u64) -> io::Result<[u8; N]> {
        let mut out = [0; N];
        let mut done = 0;
        while done < N {
            let position = at + done as u64;
            let offset = (position % PAGE as u64) as usize;
            let count = (PAGE - offset).min(N - done);
            out[done..done + count].copy_from_slice(&self.page(position / PAGE as u64)?.bytes[offset..offset + count]);
            done += count;
        }
        Ok(out)
    }
    fn write(&mut self, at: u64, bytes: &[u8]) -> io::Result<()> {
        let mut done = 0;
        while done < bytes.len() {
            let position = at + done as u64;
            let offset = (position % PAGE as u64) as usize;
            let count = (PAGE - offset).min(bytes.len() - done);
            let page = self.page(position / PAGE as u64)?;
            page.bytes[offset..offset + count].copy_from_slice(&bytes[done..done + count]);
            page.dirty = true;
            done += count;
        }
        Ok(())
    }
    fn pointer(&mut self, at: u64) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.read(at)?))
    }
}

#[derive(Default)]
pub(super) struct Cells {
    memory: Vec<(u128, Cell)>,
    root: u64,
    disk: Disk,
}

impl Cells {
    pub fn get(&mut self, key: u128) -> io::Result<Option<Cell>> {
        if self.root == 0 {
            return Ok(self.memory.binary_search_by_key(&key, |entry| entry.0).ok().map(|at| self.memory[at].1));
        }
        let Some(at) = self.slot(key, false)? else {
            return Ok(None);
        };
        let cell = Cell::of(self.disk.read(at)?);
        Ok(cell.exists().then_some(cell))
    }
    pub fn set(&mut self, key: u128, cell: Cell) -> io::Result<()> {
        if self.root == 0 {
            match self.memory.binary_search_by_key(&key, |entry| entry.0) {
                Ok(at) => {
                    self.memory[at].1 = cell;
                    return Ok(());
                }
                Err(at) if self.memory.len() < CELLS => {
                    self.memory.insert(at, (key, cell));
                    return Ok(());
                }
                Err(_) => {}
            }
        }
        if self.root == 0 {
            self.root = self.disk.allocate(PAGE as u64)?;
            for (key, cell) in std::mem::take(&mut self.memory) {
                let at = self.slot(key, true)?.expect("allocated");
                self.disk.write(at, &cell.bytes())?;
            }
        }
        let at = self.slot(key, true)?.expect("allocated");
        self.disk.write(at, &cell.bytes())
    }
    pub fn remove(&mut self, key: u128) -> io::Result<()> {
        if self.root == 0 {
            if let Ok(at) = self.memory.binary_search_by_key(&key, |entry| entry.0) {
                self.memory.remove(at);
            }
        } else if let Some(at) = self.slot(key, false)? {
            self.disk.write(at, &[0; 32])?;
        }
        Ok(())
    }
    fn slot(&mut self, key: u128, create: bool) -> io::Result<Option<u64>> {
        let page_key = (key >> 7).to_be_bytes();
        let mut node = self.root;
        for byte in page_key {
            let at = node + u64::from(byte) * 8;
            let mut next = self.disk.pointer(at)?;
            if next == 0 {
                if !create {
                    return Ok(None);
                }
                next = self.disk.allocate(PAGE as u64)?;
                self.disk.write(at, &next.to_le_bytes())?;
            }
            node = next;
        }
        Ok(Some(node + (key & 127) as u64 * 32))
    }
    pub fn next(&mut self, from: u128) -> io::Result<Option<(u128, Cell)>> {
        if self.root == 0 {
            return Ok(self.memory.get(self.memory.partition_point(|entry| entry.0 < from)).copied());
        }
        self.search(self.root, 0, 0, from, false)
    }
    pub fn previous(&mut self, through: u128) -> io::Result<Option<(u128, Cell)>> {
        if self.root == 0 {
            return Ok(self
                .memory
                .partition_point(|entry| entry.0 <= through)
                .checked_sub(1)
                .map(|at| self.memory[at]));
        }
        self.search(self.root, 0, 0, through, true)
    }
    fn search(
        &mut self,
        node: u64,
        level: usize,
        prefix: u128,
        wanted: u128,
        reverse: bool,
    ) -> io::Result<Option<(u128, Cell)>> {
        if level == 16 {
            for index in 0..128 {
                let index = if reverse { 127 - index } else { index };
                let key = (prefix << 7) | index as u128;
                if (!reverse && key < wanted) || (reverse && key > wanted) {
                    continue;
                }
                let cell = Cell::of(self.disk.read(node + index as u64 * 32)?);
                if cell.exists() {
                    return Ok(Some((key, cell)));
                }
            }
            return Ok(None);
        }
        let shift = (15 - level) * 8;
        let wanted_page = wanted >> 7;
        let target = ((wanted_page >> shift) & 255) as usize;
        let same = level == 0 || prefix == wanted_page >> (128 - level * 8);
        for index in 0..256 {
            let index = if reverse { 255 - index } else { index };
            if same && ((!reverse && index < target) || (reverse && index > target)) {
                continue;
            }
            let child = self.disk.pointer(node + index as u64 * 8)?;
            if child == 0 {
                continue;
            }
            if let Some(found) = self.search(child, level + 1, (prefix << 8) | index as u128, wanted, reverse)? {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }
    pub fn append(&mut self, cell: &mut Cell, text: &str) -> io::Result<()> {
        if cell.head == 0 {
            let bytes = cell.inline.to_le_bytes();
            let length = bytes[0].saturating_sub(1) as usize;
            let at = self.record(&bytes[1..1 + length])?;
            cell.head = at;
            cell.tail = at;
            cell.inline = 0;
        }
        let next = self.record(text.as_bytes())?;
        self.disk.write(cell.tail, &next.to_le_bytes())?;
        cell.tail = next;
        Ok(())
    }
    fn record(&mut self, text: &[u8]) -> io::Result<u64> {
        assert!(text.len() <= 4);
        let at = self.disk.allocate(16)?;
        let mut bytes = [0; 16];
        bytes[8] = text.len() as u8;
        bytes[9..9 + text.len()].copy_from_slice(text);
        self.disk.write(at, &bytes)?;
        Ok(at)
    }
    pub fn text(&mut self, cell: Cell, mut consume: impl FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        if cell.head == 0 {
            let bytes = cell.inline.to_le_bytes();
            let length = bytes[0].saturating_sub(1) as usize;
            return consume(std::str::from_utf8(&bytes[1..1 + length]).map_err(io::Error::other)?);
        }
        let mut at = cell.head;
        while at != 0 {
            let bytes: [u8; 16] = self.disk.read(at)?;
            consume(std::str::from_utf8(&bytes[9..9 + bytes[8] as usize]).map_err(io::Error::other)?)?;
            at = u64::from_le_bytes(bytes[..8].try_into().expect("pointer"));
        }
        Ok(())
    }
}
