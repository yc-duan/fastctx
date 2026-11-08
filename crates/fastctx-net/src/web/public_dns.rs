//! Where public DNS says a name leads, for a name this machine cannot see
//! the address of itself: one it cannot resolve, leaving it to the proxy, or
//! resolves only to a fake-ip proxy's own addresses. The proxy, resolving
//! the name in turn, reaches what public DNS answers, so a name public DNS
//! sends to a private address, or does not know, is not read. Asked of
//! public resolvers over HTTPS, in their JSON form, by the same way out as
//! any other request.

use std::net::IpAddr;
use std::time::Duration;

use serde::Deserialize;

/// Resolvers asked in turn, until one answers: AliDNS, which mainland China
/// reaches without a proxy, then Cloudflare.
const RESOLVERS: &[(&str, &str)] = &[
    ("https://dns.alidns.com/resolve", "application/json"),
    ("https://cloudflare-dns.com/dns-query", "application/dns-json"),
];
/// How long one resolver has to answer.
const PATIENCE: Duration = Duration::from_secs(8);
/// The records of a name's addresses: IPv4 and IPv6.
const RECORDS: [u16; 2] = [1, 28];

#[derive(Deserialize)]
struct Answered {
    #[serde(rename = "Status")]
    status: u32,
    #[serde(rename = "Answer", default)]
    answer: Vec<Record>,
}

#[derive(Deserialize)]
struct Record {
    #[serde(rename = "type")]
    kind: u16,
    data: String,
}

/// The addresses public DNS gives `host`; none when it knows no such name.
pub(super) async fn addresses(host: &str) -> Result<Vec<IpAddr>, String> {
    let mut failures = Vec::new();
    for (resolver, accept) in RESOLVERS {
        let asked = async {
            let (v4, v6) =
                tokio::join!(record(resolver, accept, host, RECORDS[0]), record(resolver, accept, host, RECORDS[1]));
            let mut found = v4?;
            found.extend(v6?);
            Ok::<_, String>(found)
        };
        match tokio::time::timeout(PATIENCE, asked).await {
            Ok(Ok(found)) => return Ok(found),
            Ok(Err(why)) => failures.push(why),
            Err(_) => failures.push(format!("{resolver} did not answer within {} s", PATIENCE.as_secs())),
        }
    }
    Err(failures.join("; "))
}

/// The addresses of one kind of record `resolver` gives `host`.
async fn record(resolver: &str, accept: &str, host: &str, kind: u16) -> Result<Vec<IpAddr>, String> {
    let response = super::get(&format!("{resolver}?name={host}&type={kind}"), accept, 0).await?;
    if response.status != 200 {
        return Err(format!("{resolver} answered {}", response.status));
    }
    let body = response.bytes(256 * 1024).await?;
    let answered: Answered = serde_json::from_slice(&body).map_err(|error| format!("{resolver}: {error}"))?;
    match answered.status {
        0 => Ok(answered
            .answer
            .iter()
            .filter(|record| record.kind == kind)
            .filter_map(|record| record.data.parse().ok())
            .collect()),
        // The name does not exist.
        3 => Ok(Vec::new()),
        status => Err(format!("{resolver} could not resolve {host} (DNS status {status})")),
    }
}
