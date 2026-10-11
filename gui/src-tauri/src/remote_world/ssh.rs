//! The SSH side of making a World on a server, with nothing of the app's
//! shell in it (`mod.rs` is that). The connection is made in this process;
//! the password or key given is used for that one connection and never kept
//! or written anywhere. A first connection goes only as far as the server's
//! key, for the person to confirm; the connection that does the work is
//! refused if the server shows any other key than the one confirmed.
//!
//! The work: the Linux program goes up to a temporary directory on the
//! server and is checked by its digest; it makes the hub there as a system
//! service (`fastctx hub init --service`, the administrator's password on
//! its standard input, never on a command line); later, with a code made
//! once signed in, it installs the server as one of the World's machines
//! (`fastctx server install`) and the directory goes.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use russh::client::{self, Handle};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// What the page reads when sudo wants a password it was not given
/// (`gui/src/app/remote-world.ts`).
pub const SUDO_WANTED: &str = "sudo needs a password";

/// A server the hub was made on, its session open for it to join.
pub struct Remote {
    handle: Handle<Checked>,
    /// The temporary directory the program was unpacked in.
    dir: String,
    sudo: Sudo,
}

/// How a command gets root on the server.
#[derive(Clone)]
enum Sudo {
    /// Signed in as root.
    Root,
    /// sudo without a password.
    Free,
    /// sudo with this password, on the first line of the command's input.
    Password(String),
}

impl Sudo {
    fn prefix(&self) -> &'static str {
        match self {
            Sudo::Root => "",
            Sudo::Free => "sudo -n -H ",
            // `-k` makes sudo always read the password, so the line meant for
            // it never reaches the command.
            Sudo::Password(_) => "sudo -S -k -p '' -H ",
        }
    }

    fn input(&self, rest: &str) -> Vec<u8> {
        match self {
            Sudo::Password(password) => format!("{password}\n{rest}").into_bytes(),
            _ => rest.as_bytes().to_vec(),
        }
    }
}

/// Keeps the key the server shows, and accepts only the expected one.
struct Checked {
    expected: Option<String>,
    seen: Arc<Mutex<Option<PublicKey>>>,
}

impl client::Handler for Checked {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let key = key.public_key();
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        *self.seen.lock().expect("never poisoned") = Some(key);
        Ok(self.expected.as_deref() == Some(fingerprint.as_str()))
    }
}

/// The server as it showed itself: where the connection went, its key's
/// fingerprint, and whether this computer's SSH records know that key.
#[derive(Serialize, Debug)]
pub struct Probed {
    pub address: String,
    pub fingerprint: String,
    pub known: &'static str,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Auth {
    Password { password: String },
    Key { key: String, passphrase: Option<String> },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Auth,
    pub fingerprint: String,
    pub sudo: Option<String>,
    pub world: String,
    pub admin: String,
    pub password: String,
    pub hub_port: u16,
}

/// The hub made: where it is reached, as its public address, and the
/// server's own name.
#[derive(Serialize, Debug)]
pub struct Made {
    pub address: String,
    pub hostname: String,
}

/// Connects far enough to see the server's key, and says whether this
/// computer's SSH records know it.
pub async fn probe(host: &str, port: u16) -> Result<Probed, String> {
    let seen = Arc::new(Mutex::new(None));
    let (address, connected) = connect(host, port, Checked { expected: None, seen: seen.clone() }).await?;
    if let Ok(handle) = connected {
        // No key is expected here, so a session never opens; if one did, it
        // ends at once.
        let _ = handle.disconnect(Disconnect::ByApplication, "", "").await;
    }
    let key = seen.lock().expect("never poisoned").take().ok_or_else(|| format!("{host} showed no key"))?;
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    let known = match russh::keys::check_known_hosts(host, port, &key) {
        Ok(true) => "known",
        Ok(false) => "unknown",
        Err(russh::keys::Error::KeyChanged { .. }) => "changed",
        Err(russh::keys::Error::NoHomeDir) => "unknown",
        Err(error) => return Err(format!("reading this computer's known_hosts: {error}")),
    };
    Ok(Probed { address: address.to_string(), fingerprint, known })
}

/// Connects, checks the server, uploads `archive` and makes the hub, saying
/// each step as it begins; the session stays open for the server to join.
pub async fn make_hub(request: &Request, archive: &Path, step: impl Fn(&'static str) -> Result<(), String>) -> Result<(Remote, Made), String> {
    step("connect")?;
    let seen = Arc::new(Mutex::new(None));
    let checked = Checked { expected: Some(request.fingerprint.clone()), seen };
    let (address, connected) = connect(&request.host, request.port, checked).await?;
    let mut handle = connected.map_err(|error| match error {
        russh::Error::UnknownKey => format!("{} showed another key than the one confirmed", request.host),
        error => format!("connecting to {}: {error}", request.host),
    })?;
    let accepted = match &request.auth {
        Auth::Password { password } => handle.authenticate_password(&request.user, password).await,
        Auth::Key { key, passphrase } => {
            let key = russh::keys::decode_secret_key(key, passphrase.as_deref()).map_err(|error| format!("reading the key file: {error}"))?;
            let hash = handle.best_supported_rsa_hash().await.map_err(|error| error.to_string())?.flatten();
            handle.authenticate_publickey(&request.user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await
        }
    }
    .map_err(|error| format!("signing in to {}: {error}", request.host))?;
    if !accepted.success() {
        return Err(format!("{} did not accept {} with this password or key", request.host, request.user));
    }

    step("check")?;
    let facts = run(&handle, "uname -s; uname -m; id -u; hostname", Vec::new()).await?.ok("looking at the server")?;
    let facts = String::from_utf8_lossy(&facts);
    let mut lines = facts.lines();
    let (system, machine, uid, hostname) = (lines.next(), lines.next(), lines.next(), lines.next());
    if system != Some("Linux") {
        return Err(format!("the server runs {}, not Linux", system.unwrap_or("an unknown system")));
    }
    if machine != Some("x86_64") {
        return Err(format!("the server is {}; this app carries the program for x86_64 only", machine.unwrap_or("of an unknown architecture")));
    }
    let sudo = if uid == Some("0") {
        Sudo::Root
    } else if run(&handle, "sudo -n true", Vec::new()).await?.status == Some(0) {
        Sudo::Free
    } else {
        match &request.sudo {
            Some(password) => Sudo::Password(password.clone()),
            None => return Err(SUDO_WANTED.to_string()),
        }
    };
    if let Sudo::Password(_) = &sudo {
        run(&handle, &format!("{}true", sudo.prefix()), sudo.input("")).await?.ok("sudo")?;
    }

    step("upload")?;
    let digest = sha256(archive).await?;
    let dir = run(&handle, "d=$(mktemp -d) && cat > \"$d/fastctx.tgz\" && printf %s \"$d\"", Upload::File(archive.to_path_buf()))
        .await?
        .ok("uploading the program")?;
    let dir = String::from_utf8(dir).map_err(|_| "the server named its temporary directory oddly".to_string())?;
    run(
        &handle,
        &format!("cd {} && printf '%s  fastctx.tgz\\n' {digest} | sha256sum -c --quiet - && tar xzf fastctx.tgz && rm fastctx.tgz", quote(&dir)),
        Vec::new(),
    )
    .await?
    .ok("checking the program on the server")?;

    step("hub")?;
    let public = SocketAddr::new(address.ip(), request.hub_port);
    run(
        &handle,
        &format!(
            "cd {} && {}./fastctx hub init --name {} --admin {} --public {} --service",
            quote(&dir),
            sudo.prefix(),
            quote(&request.world),
            quote(&request.admin),
            quote(&public.to_string())
        ),
        sudo.input(&format!("{}\n", request.password)),
    )
    .await?
    .ok("making the World on the server")?;
    let hostname = hostname.unwrap_or_default().trim().to_string();
    Ok((Remote { handle, dir, sudo }, Made { address: public.to_string(), hostname }))
}

impl Remote {
    /// Installs the server as one of the World's machines with `token`.
    pub async fn join(&self, token: &str) -> Result<(), String> {
        run(&self.handle, &format!("cd {} && {}./fastctx server install {}", quote(&self.dir), self.sudo.prefix(), quote(token)), self.sudo.input(""))
            .await?
            .ok("adding the server to the World")?;
        Ok(())
    }

    /// Takes the temporary directory away and ends the session.
    pub async fn close(&self) -> Result<(), String> {
        run(&self.handle, &format!("rm -rf {}", quote(&self.dir)), Vec::new()).await?.ok("tidying the server")?;
        self.handle.disconnect(Disconnect::ByApplication, "", "").await.map_err(|error| error.to_string())
    }
}

/// Opens the TCP connection itself, so the address it reached is known, and
/// starts SSH over it.
async fn connect(host: &str, port: u16, checked: Checked) -> Result<(SocketAddr, Result<Handle<Checked>, russh::Error>), String> {
    let stream = tokio::net::TcpStream::connect((host, port)).await.map_err(|error| format!("reaching {host}:{port}: {error}"))?;
    let address = stream.peer_addr().map_err(|error| error.to_string())?;
    stream.set_nodelay(true).map_err(|error| error.to_string())?;
    let config = Arc::new(client::Config::default());
    Ok((address, client::connect_stream(config, stream, checked).await))
}

async fn sha256(path: &Path) -> Result<String, String> {
    use tokio::io::AsyncReadExt as _;
    let mut file = tokio::fs::File::open(path).await.map_err(|error| format!("reading {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = file.read(&mut buffer).await.map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

/// What a command's standard input gets.
enum Upload {
    Bytes(Vec<u8>),
    File(PathBuf),
}

impl From<Vec<u8>> for Upload {
    fn from(bytes: Vec<u8>) -> Self {
        Upload::Bytes(bytes)
    }
}

struct Ran {
    status: Option<u32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

impl Ran {
    /// The output of a command that succeeded; one that did not says what
    /// it was doing and what it wrote.
    fn ok(self, doing: &str) -> Result<Vec<u8>, String> {
        if self.status == Some(0) {
            return Ok(self.out);
        }
        let said = String::from_utf8_lossy(if self.err.is_empty() { &self.out } else { &self.err }).trim().to_string();
        Err(match self.status {
            Some(code) => format!("{doing}: exit {code}: {said}"),
            None => format!("{doing}: the command ended without a status: {said}"),
        })
    }
}

/// Runs one command, writing its input while reading its output, since a
/// full output buffer would otherwise stop the input.
async fn run(handle: &Handle<Checked>, command: &str, input: impl Into<Upload>) -> Result<Ran, String> {
    let channel = handle.channel_open_session().await.map_err(|error| error.to_string())?;
    channel.exec(true, command).await.map_err(|error| error.to_string())?;
    let (mut reading, writing) = channel.split();
    let write = async {
        match input.into() {
            Upload::Bytes(bytes) => writing.data(&bytes[..]).await.map_err(|error| error.to_string())?,
            Upload::File(path) => {
                let file = tokio::fs::File::open(&path).await.map_err(|error| format!("reading {}: {error}", path.display()))?;
                writing.data(file).await.map_err(|error| error.to_string())?;
            }
        }
        writing.eof().await.map_err(|error| error.to_string())
    };
    let read = async {
        let mut ran = Ran { status: None, out: Vec::new(), err: Vec::new() };
        while let Some(message) = reading.wait().await {
            match message {
                ChannelMsg::Data { data } => ran.out.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, ext: 1 } => ran.err.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => ran.status = Some(exit_status),
                _ => {}
            }
        }
        ran
    };
    let (written, ran) = tokio::join!(write, read);
    written?;
    Ok(ran)
}

/// A word for a POSIX shell, single-quoted.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
