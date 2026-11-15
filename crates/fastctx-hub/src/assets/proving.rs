//! Proofs of holding by challenge (design §6.5, §28.45). A device the hub
//! asked to prove it holds an object for a frame it sent may answer a
//! one-time nonce instead of sending every byte again: the hub reads its own
//! bytes and works out `SHA256(PROOF_TAG ‖ nonce ‖ bytes)` alike, with the
//! object's own hash in the same reading, on two threads of its own, the
//! rest queued in the order they came. Bytes that are not the object's are
//! found so and the object goes back to wanted, to be sent again whole.
//!
//! What it may cost is bounded: a device has at most two challenges not
//! settled; an answer that is not the hub's bars that object to that device
//! for ten minutes, and the third in an hour every challenge for an hour;
//! the hub holds at most so many objects asked and challenges at once. An
//! answer is settled apart from the request that brought it and asked after
//! again, so no call waits on the reading; asking with no answer keeps what
//! the device proved and staged meanwhile. What is proved is kept as a
//! whole sending keeps it: under the device's key, by the hash, for any
//! frame that refers to the object (§28.45).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use fastctx_content::v2::Library;
use fastctx_types::assets::{CHALLENGES, PROOF_TAG, ProofState};
use sha2::{Digest, Sha256};

/// How long the hub's asking a device to prove an object stands.
const ASKED_FOR: Duration = Duration::from_secs(600);
/// How long a challenge stands before the time to read its object.
const ANSWER_FOR: Duration = Duration::from_secs(600);
/// How long a settled answer is kept to be asked after.
const KEPT_FOR: Duration = Duration::from_secs(600);
/// How long a wrong answer bars that object to that device.
const BARRED_FOR: Duration = Duration::from_secs(600);
/// Wrong answers a device may give in `WRONG_WINDOW`; the next bars it
/// from every challenge for `BANNED_FOR`.
const WRONG_MOST: usize = 2;
const WRONG_WINDOW: Duration = Duration::from_secs(3600);
const BANNED_FOR: Duration = Duration::from_secs(3600);
/// Challenges a device may have that are not settled.
const UNSETTLED_MOST: usize = 2;
/// Objects a device may be asked to prove at once, and the hub in all.
const ASKED_MOST: usize = 4 * fastctx_content::FRAME_OBJECTS;
const ASKED_ALL: usize = 1 << 16;
/// Challenges the hub holds at once, settled ones kept to be asked after
/// among them.
const NONCES_ALL: usize = 1 << 12;
/// Proofs worked out at once.
const WORKERS: usize = 2;
/// The rate a challenge allows for reading its object (§6.5).
const READ_RATE: u64 = 50 * 1024 * 1024;
/// Bytes read at a time.
const PIECE: u32 = 1024 * 1024;

/// How a challenge stands, and what it proved, the first time it is told so.
type Answer = (ProofState, Option<([u8; 32], u64)>);

/// What is asked and proved by challenge, and the threads that work it out.
#[derive(Default)]
pub(crate) struct Proving {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    queue: Mutex<Queue>,
    woken: Condvar,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    workers: usize,
}

struct Job {
    nonce: [u8; 32],
    library: Library,
    hash: [u8; 32],
    size: u64,
    answer: [u8; 32],
}

#[derive(Default)]
struct State {
    /// By device key, the asset sync it opened its session in.
    versions: HashMap<[u8; 32], u32>,
    /// By device key, the objects the hub asked it lately to prove.
    asked: HashMap<[u8; 32], HashMap<[u8; 32], Asked>>,
    nonces: HashMap<[u8; 32], Nonce>,
    /// By device key, when it answered wrong lately.
    wrong: HashMap<[u8; 32], Vec<Instant>>,
    /// Devices barred from every challenge until when.
    banned: HashMap<[u8; 32], Instant>,
    /// Device and object barred until when.
    barred: HashMap<([u8; 32], [u8; 32]), Instant>,
}

struct Asked {
    scope: String,
    size: u64,
    until: Instant,
}

struct Nonce {
    device: [u8; 32],
    hash: [u8; 32],
    size: u64,
    scope: String,
    until: Instant,
    answer: Option<[u8; 32]>,
    state: ProofState,
    settled: Option<Instant>,
    /// Whether what it proved was kept among the device's proofs.
    kept: bool,
}

impl Nonce {
    fn unsettled(&self, now: Instant) -> bool {
        match self.answer {
            None => now < self.until,
            Some(_) => self.settled.is_none(),
        }
    }
}

impl State {
    /// Lets go of what stands no longer.
    fn tidy(&mut self, now: Instant) {
        self.nonces.retain(|_, nonce| match (nonce.answer, nonce.settled) {
            (None, _) => now < nonce.until,
            (Some(_), None) => true,
            (Some(_), Some(settled)) => now.duration_since(settled) < KEPT_FOR,
        });
        self.barred.retain(|_, until| now < *until);
        self.banned.retain(|_, until| now < *until);
        self.wrong.retain(|_, at| {
            at.retain(|at| now.duration_since(*at) < WRONG_WINDOW);
            !at.is_empty()
        });
        self.asked.retain(|_, asked| {
            asked.retain(|_, asked| now < asked.until);
            !asked.is_empty()
        });
    }

    fn asked_all(&self) -> usize {
        self.asked.values().map(HashMap::len).sum()
    }
}

impl Proving {
    /// The device `device` opened its session in asset sync `version`.
    pub(crate) fn opened(&self, device: &[u8; 32], version: u32) {
        self.inner.state.lock().expect("proving").versions.insert(*device, version);
    }

    /// The asset sync version the device `device` opened its session in.
    pub(crate) fn version(&self, device: &[u8; 32]) -> Option<u32> {
        self.inner.state.lock().expect("proving").versions.get(device).copied()
    }

    /// The hub asked the device `device` to prove it holds `objects` (each
    /// its hash and length) of `scope`, for a frame it sent: within the
    /// device's bound and the hub's, one past them proved by sending it.
    pub(crate) fn asked(&self, device: &[u8; 32], scope: &str, objects: &[([u8; 32], u64)]) {
        let now = Instant::now();
        let mut state = self.inner.state.lock().expect("proving");
        state.tidy(now);
        let mut room = ASKED_ALL.saturating_sub(state.asked_all());
        let asked = state.asked.entry(*device).or_default();
        for (hash, size) in objects {
            let known = asked.contains_key(hash);
            if !known && (asked.len() >= ASKED_MOST || room == 0) {
                break;
            }
            if !known {
                room -= 1;
            }
            asked.insert(*hash, Asked { scope: scope.to_string(), size: *size, until: now + ASKED_FOR });
        }
    }

    /// The scope the device `device` was asked lately to prove it holds the
    /// object `hash` of `size` for, if it may be challenged for it now.
    pub(crate) fn asked_scope(&self, device: &[u8; 32], hash: &[u8; 32], size: u64) -> Result<String, String> {
        let now = Instant::now();
        let mut state = self.inner.state.lock().expect("proving");
        state.tidy(now);
        if state.versions.get(device).is_none_or(|version| *version < CHALLENGES) {
            return Err(format!("challenges come with asset sync {CHALLENGES}: prove it by sending it"));
        }
        let asked = state.asked.get(device).and_then(|asked| asked.get(hash)).filter(|asked| asked.size == size);
        let Some(asked) = asked else {
            return Err("this machine was not asked lately to prove it holds that object: prove it by sending it".into());
        };
        let scope = asked.scope.clone();
        challengeable(&state, device, hash, now)?;
        Ok(scope)
    }

    /// A challenge for the device `device` to prove it holds the object
    /// `hash` of `size` of `scope`: its nonce, and until when it stands.
    pub(crate) fn issue(&self, device: &[u8; 32], hash: &[u8; 32], size: u64, scope: &str) -> Result<([u8; 32], Instant), String> {
        let nonce = fastctx_net::sign::random::<32>().map_err(|error| error.to_string())?;
        let now = Instant::now();
        let mut state = self.inner.state.lock().expect("proving");
        state.tidy(now);
        challengeable(&state, device, hash, now)?;
        if state.nonces.len() >= NONCES_ALL {
            // The settled kept longest go first; with none settled, none is
            // given.
            let oldest = state
                .nonces
                .iter()
                .filter_map(|(key, nonce)| nonce.settled.map(|settled| (settled, *key)))
                .min()
                .map(|(_, key)| key);
            match oldest {
                Some(key) => {
                    state.nonces.remove(&key);
                }
                None => return Err("the hub has as many challenges in hand as it holds: prove it by sending it".into()),
            }
        }
        let until = now + ANSWER_FOR + Duration::from_secs(size / READ_RATE);
        state.nonces.insert(
            nonce,
            Nonce {
                device: *device,
                hash: *hash,
                size,
                scope: scope.to_string(),
                until,
                answer: None,
                state: ProofState::Working,
                settled: None,
                kept: false,
            },
        );
        Ok((nonce, until))
    }

    /// How the challenge `nonce` of the device `device` stands, its answer
    /// not given: asked by a device that only keeps what it proved and
    /// staged while it reads.
    pub(crate) fn standing(&self, device: &[u8; 32], nonce: &[u8; 32]) -> Result<ProofState, String> {
        let state = self.inner.state.lock().expect("proving");
        match state.nonces.get(nonce) {
            Some(kept) if kept.device == *device => Ok(kept.state),
            Some(_) => Err("that challenge is another machine's".into()),
            None => Err("the hub knows no such challenge (it may have restarted): send the frame again".into()),
        }
    }

    /// The device `device` answers the challenge `nonce` with `answer`, or
    /// asks again how it stands: how it stands, and, the first time it is
    /// told proved, the object to keep among the device's proofs. The
    /// library the object is read from is `library`'s for its scope.
    pub(crate) fn answer(
        &self,
        device: &[u8; 32],
        nonce: &[u8; 32],
        answer: &[u8; 32],
        library: impl FnOnce(&str) -> Result<Library, String>,
    ) -> Result<Answer, String> {
        // Judged as it came, whatever waits after (S01), and taken then: no
        // tidying meanwhile lets it go.
        let came = Instant::now();
        let (scope, hash, size) = {
            let mut state = self.inner.state.lock().expect("proving");
            let Some(kept) = state.nonces.get_mut(nonce) else {
                return Err("the hub knows no such challenge (it may have restarted): send the frame again".into());
            };
            if kept.device != *device {
                return Err("that challenge is another machine's".into());
            }
            match kept.answer {
                Some(given) if given != *answer => return Err("a challenge is answered once".into()),
                Some(_) => {
                    let proved = (kept.state == ProofState::Proven && !kept.kept).then_some((kept.hash, kept.size));
                    if proved.is_some() {
                        kept.kept = true;
                    }
                    return Ok((kept.state, proved));
                }
                None if came >= kept.until => {
                    state.nonces.remove(nonce);
                    return Err("that challenge ended before it was answered: send the frame again".into());
                }
                None => {
                    kept.answer = Some(*answer);
                    (kept.scope.clone(), kept.hash, kept.size)
                }
            }
        };
        match library(&scope) {
            Ok(library) => self.queue(Job { nonce: *nonce, library, hash, size, answer: *answer }),
            // Its scope gone meanwhile: settled as one to send instead.
            Err(_) => settle(&self.inner, nonce, ProofState::Damaged),
        }
        Ok((ProofState::Working, None))
    }

    fn queue(&self, job: Job) {
        let mut queue = self.inner.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if queue.workers < WORKERS {
            let inner = self.inner.clone();
            match std::thread::Builder::new().name("proofs".into()).spawn(move || work(&inner)) {
                Ok(_) => queue.workers += 1,
                // No thread at all to work it out: settled as one to send.
                Err(_) if queue.workers == 0 => {
                    drop(queue);
                    settle(&self.inner, &job.nonce, ProofState::Damaged);
                    return;
                }
                Err(_) => {}
            }
        }
        queue.jobs.push_back(job);
        drop(queue);
        self.inner.woken.notify_one();
    }
}

/// Whether the device `device` may be challenged for the object `hash` now.
fn challengeable(state: &State, device: &[u8; 32], hash: &[u8; 32], now: Instant) -> Result<(), String> {
    if state.banned.contains_key(device) {
        return Err("this machine answered too many challenges wrong lately: prove what it holds by sending it".into());
    }
    if state.barred.contains_key(&(*device, *hash)) {
        return Err("this machine answered a challenge for that object wrong lately: prove it by sending it".into());
    }
    let unsettled = state.nonces.values().filter(|nonce| nonce.device == *device && nonce.unsettled(now)).count();
    if unsettled >= UNSETTLED_MOST {
        return Err(format!("this machine has {unsettled} challenges not settled: prove it by sending it"));
    }
    Ok(())
}

/// Settles the challenge `nonce` as `proof`; a wrong answer bars its object
/// to its device, and the third in an hour the device itself, for an hour.
fn settle(inner: &Inner, nonce: &[u8; 32], proof: ProofState) {
    let now = Instant::now();
    let mut state = inner.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(kept) = state.nonces.get_mut(nonce) else { return };
    kept.state = proof;
    kept.settled = Some(now);
    let (device, hash) = (kept.device, kept.hash);
    if proof == ProofState::Wrong {
        state.barred.insert((device, hash), now + BARRED_FOR);
        let wrong = state.wrong.entry(device).or_default();
        wrong.retain(|at| now.duration_since(*at) < WRONG_WINDOW);
        wrong.push(now);
        if wrong.len() > WRONG_MOST {
            state.banned.insert(device, now + BANNED_FOR);
        }
    }
}

/// One of the threads that work out proofs, for as long as the hub runs: a
/// proof whose working out fails is settled as one to send instead.
fn work(inner: &Inner) {
    loop {
        let job = {
            let mut queue = inner.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if let Some(job) = queue.jobs.pop_front() {
                    break job;
                }
                queue = inner.woken.wait(queue).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        let proof = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prove(&job))).unwrap_or(ProofState::Damaged);
        if proof == ProofState::Damaged {
            // Not the object's: wanted again, and the frames that refer to
            // it hear it missing until it is sent whole.
            let _ = job.library.damaged(&job.hash);
        }
        settle(inner, &job.nonce, proof);
    }
}

/// Reads the hub's own bytes of the job's object once: the answer is right
/// where they work out to it, and the bytes are the object's where they
/// work out to its hash.
fn prove(job: &Job) -> ProofState {
    let mut tagged = Sha256::new();
    tagged.update(PROOF_TAG);
    tagged.update(job.nonce);
    let mut plain = Sha256::new();
    let mut offset = 0;
    while offset < job.size {
        let length = (job.size - offset).min(u64::from(PIECE)) as u32;
        let bytes = match job.library.read(&job.hash, offset, length) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            _ => return ProofState::Damaged,
        };
        tagged.update(&bytes);
        plain.update(&bytes);
        offset += bytes.len() as u64;
    }
    if <[u8; 32]>::from(plain.finalize()) != job.hash {
        return ProofState::Damaged;
    }
    if <[u8; 32]>::from(tagged.finalize()) == job.answer { ProofState::Proven } else { ProofState::Wrong }
}

#[cfg(test)]
mod measure {
    //! The §18 row for a proof by challenge (design §28.45): an object of
    //! `FASTCTX_PROOF_MIB` (1024 by default) held by the hub and the device
    //! alike, from the challenge to the hub's word that it is proved — the
    //! device's reading and the hub's — once, then two at once. Run with
    //! `cargo test -p fastctx-hub --release proving::measure -- --ignored --nocapture`.
    use super::*;
    use std::io::Read;

    struct Pattern(u64, u64);

    impl Read for Pattern {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min((self.1 - self.0) as usize);
            for (at, byte) in buf[..n].iter_mut().enumerate() {
                let i = self.0 + at as u64;
                *byte = (i ^ (i >> 13) ^ (i >> 29)) as u8;
            }
            self.0 += n as u64;
            Ok(n)
        }
    }

    fn answer(library: &Library, hash: &[u8; 32], size: u64, nonce: &[u8; 32]) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(PROOF_TAG);
        digest.update(nonce);
        let mut offset = 0;
        while offset < size {
            let bytes = library.read(hash, offset, (size - offset).min(u64::from(PIECE)) as u32).unwrap();
            digest.update(&bytes);
            offset += bytes.len() as u64;
        }
        digest.finalize().into()
    }

    fn one(proving: &Proving, library: &Library, device: [u8; 32], hash: [u8; 32], size: u64) -> Duration {
        let began = Instant::now();
        proving.opened(&device, CHALLENGES);
        proving.asked(&device, "s", &[(hash, size)]);
        let scope = proving.asked_scope(&device, &hash, size).unwrap();
        let (nonce, _) = proving.issue(&device, &hash, size, &scope).unwrap();
        let given = answer(library, &hash, size, &nonce);
        loop {
            let (state, _) = proving.answer(&device, &nonce, &given, |_| Ok(library.clone())).unwrap();
            match state {
                ProofState::Working => std::thread::sleep(Duration::from_millis(20)),
                ProofState::Proven => return began.elapsed(),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    #[ignore]
    fn proof_by_challenge() {
        let mib: u64 = std::env::var("FASTCTX_PROOF_MIB").ok().and_then(|mib| mib.parse().ok()).unwrap_or(1024);
        let size = mib * 1024 * 1024;
        let root = std::env::temp_dir().join(format!("fastctx-proof-{}", std::process::id()));
        let (library, _) = Library::open(&root).unwrap();
        let lease = library.put(&mut Pattern(0, size), "measure", Duration::from_secs(3600)).unwrap();
        let (hash, size) = (lease.object.hash, lease.object.size);
        let proving = Proving::default();
        let alone = one(&proving, &library, [1; 32], hash, size);
        let both: Vec<Duration> = std::thread::scope(|scope| {
            let runs: Vec<_> = [[2u8; 32], [3u8; 32]]
                .into_iter()
                .map(|device| {
                    let (proving, library) = (&proving, &library);
                    scope.spawn(move || one(proving, library, device, hash, size))
                })
                .collect();
            runs.into_iter().map(|run| run.join().unwrap()).collect()
        });
        println!(
            "{{\"row\": \"proof by challenge\", \"mib\": {mib}, \"alone_s\": {:.2}, \"both_s\": [{:.2}, {:.2}], \"target_alone_s\": 3, \"target_both_s\": 6}}",
            alone.as_secs_f64(),
            both[0].as_secs_f64(),
            both[1].as_secs_f64()
        );
        drop(library);
        let _ = fastctx_platform::disk::remove_dir_all(&root);
    }
}
