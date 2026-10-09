//! Constant-state tracking of control sequences still open at the snapshot end.

#[derive(Clone, Copy)]
enum Kind {
    Osc,
    Apc,
    Kitty,
    Dcs,
    Csi,
}

#[derive(Clone, Copy)]
enum State {
    Ground,
    Escape { at: u64, line: u64 },
    String { kind: Kind, at: u64, line: u64, escape: bool, first: bool },
    Csi { at: u64, line: u64, intermediate: bool },
}

pub(super) struct Tracker {
    state: State,
    bytes: u64,
    line: u64,
}
impl Default for Tracker {
    fn default() -> Self {
        Self { state: State::Ground, bytes: 0, line: 1 }
    }
}

pub(super) struct Unfinished {
    kind: Kind,
    pub line: u64,
    at: u64,
    bytes: u64,
}
impl Unfinished {
    pub fn description(&self) -> String {
        let name = match self.kind {
            Kind::Osc => "OSC control",
            Kind::Apc => "APC control",
            Kind::Kitty => "kitty graphics (APC)",
            Kind::Dcs => "DCS control",
            Kind::Csi => "CSI control",
        };
        let missing = match self.kind {
            Kind::Csi => "no final byte",
            _ => "no closing delimiter",
        };
        format!(
            "unfinished {name} sequence at byte {} ({} bytes); {missing} in the output read",
            self.at + 1,
            self.bytes
        )
    }
}

impl Tracker {
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut at = 0;
        while at < bytes.len() {
            if matches!(self.state, State::Ground) {
                let count = memchr::memchr(0x1b, &bytes[at..]).unwrap_or(bytes.len() - at);
                self.line += memchr::memchr_iter(b'\n', &bytes[at..at + count]).count() as u64;
                at += count;
                if at == bytes.len() {
                    break;
                }
            }
            let byte = bytes[at];
            let offset = self.bytes + at as u64;
            self.state = match self.state {
                State::Ground => State::Escape { at: offset, line: self.line },
                State::Escape { at, line } => match byte {
                    b']' => State::String { kind: Kind::Osc, at, line, escape: false, first: true },
                    b'_' => State::String { kind: Kind::Apc, at, line, escape: false, first: true },
                    b'P' => State::String { kind: Kind::Dcs, at, line, escape: false, first: true },
                    b'[' => State::Csi { at, line, intermediate: false },
                    0x1b => State::Escape { at: offset, line: self.line },
                    _ => State::Ground,
                },
                State::Csi { at, line, intermediate } => match byte {
                    0x30..=0x3f if !intermediate => State::Csi { at, line, intermediate },
                    0x20..=0x2f => State::Csi { at, line, intermediate: true },
                    0x1b => State::Escape { at: offset, line: self.line },
                    _ => State::Ground,
                },
                State::String { mut kind, at, line, escape, first } => {
                    if first && matches!(kind, Kind::Apc) && byte == b'G' {
                        kind = Kind::Kitty;
                    }
                    if (escape && byte == b'\\')
                        || matches!(byte, 0x18 | 0x1a)
                        || (matches!(kind, Kind::Osc) && byte == 0x07)
                    {
                        State::Ground
                    } else if escape && !matches!(kind, Kind::Dcs) {
                        // OSC/APC parsing restarts at the ESC that interrupted it.
                        self.state = State::Escape { at: offset - 1, line: self.line };
                        continue;
                    } else {
                        State::String { kind, at, line, escape: byte == 0x1b, first: false }
                    }
                }
            };
            self.line += u64::from(byte == b'\n');
            at += 1;
        }
        self.bytes += bytes.len() as u64;
    }

    pub fn unfinished(&self) -> Option<Unfinished> {
        let (kind, at, line) = match self.state {
            State::String { kind, at, line, .. } => (kind, at, line),
            State::Csi { at, line, .. } => (Kind::Csi, at, line),
            _ => return None,
        };
        Some(Unfinished { kind, line, at, bytes: self.bytes - at })
    }
}
