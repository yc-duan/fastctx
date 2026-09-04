//! A resolver small enough to own: A and AAAA queries over sockets pinned to the physical
//! interface, so a TUN proxy's fake-IP resolver never answers for the hub.

use super::netpath::{Interface, is_fake_ip, pin_socket};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const QUERY_TIMEOUT: Duration = Duration::from_secs(2);
const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;
const TYPE_CNAME: u16 = 5;
const CLASS_IN: u16 = 1;

/// Resolves `host` through `interface`'s resolvers. IPv4 answers come first; IPv6 is used
/// only when IPv4 gives nothing. Fake-IP answers are reported as `dns_intercepted`.
pub(crate) async fn resolve(host: &str, interface: &Interface) -> Result<Vec<IpAddr>, String> {
    if let Ok(address) = host.parse::<IpAddr>() {
        return Ok(vec![address]);
    }
    let resolvers = interface.resolvers();
    if resolvers.is_empty() {
        return Err(format!(
            "dns_unavailable: \"{}\" names no DNS server and has no gateway to ask.",
            interface.name
        ));
    }
    let mut last_error = String::new();
    for record_type in [TYPE_A, TYPE_AAAA] {
        for resolver in &resolvers {
            let ipv6_resolver = resolver.is_ipv6();
            if ipv6_resolver && interface.ipv6.is_empty()
                || !ipv6_resolver && interface.ipv4.is_empty()
            {
                continue;
            }
            match query(host, record_type, *resolver, interface).await {
                Ok(answers) => {
                    let (fake, real): (Vec<_>, Vec<_>) = answers
                        .into_iter()
                        .partition(|address| is_fake_ip(*address));
                    if !real.is_empty() {
                        return Ok(real);
                    }
                    if !fake.is_empty() {
                        return Err(format!(
                            "dns_intercepted: {resolver} answered {} for \"{host}\", a fake-IP range that only a TUN proxy can route. Use an IP literal in the hub address or the system network mode.",
                            fake[0]
                        ));
                    }
                }
                Err(error) => last_error = error,
            }
        }
    }
    if last_error.is_empty() {
        Err(format!(
            "dns_no_answer: no resolver on \"{}\" returned an address for \"{host}\".",
            interface.name
        ))
    } else {
        Err(format!("dns_failed: {last_error}"))
    }
}

async fn query(
    host: &str,
    record_type: u16,
    resolver: IpAddr,
    interface: &Interface,
) -> Result<Vec<IpAddr>, String> {
    let id = u16::from_be_bytes([
        crate::world::crypto::random_bytes::<1>()?[0],
        crate::world::crypto::random_bytes::<1>()?[0],
    ]);
    let request = build_query(id, host, record_type)?;
    let server = SocketAddr::new(resolver, 53);
    let response = tokio::time::timeout(QUERY_TIMEOUT, udp_exchange(&request, server, interface))
        .await
        .map_err(|_| format!("{resolver} did not answer within 2 s"))??;
    let parsed = parse_response(&response, id)?;
    if parsed.truncated {
        let response =
            tokio::time::timeout(QUERY_TIMEOUT, tcp_exchange(&request, server, interface))
                .await
                .map_err(|_| format!("{resolver} did not answer over TCP within 2 s"))??;
        return parse_response(&response, id).map(|parsed| parsed.addresses);
    }
    Ok(parsed.addresses)
}

async fn udp_exchange(
    request: &[u8],
    server: SocketAddr,
    interface: &Interface,
) -> Result<Vec<u8>, String> {
    let domain = if server.is_ipv6() {
        socket2::Domain::IPV6
    } else {
        socket2::Domain::IPV4
    };
    let socket = socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))
        .map_err(|error| format!("cannot create a DNS socket: {error}"))?;
    pin_socket(
        &socket2::SockRef::from(&socket),
        interface,
        server.is_ipv6(),
    )?;
    socket
        .set_nonblocking(true)
        .map_err(|error| format!("cannot make the DNS socket non-blocking: {error}"))?;
    let socket = tokio::net::UdpSocket::from_std(socket.into())
        .map_err(|error| format!("cannot register the DNS socket: {error}"))?;
    socket
        .connect(server)
        .await
        .map_err(|error| format!("cannot address {server}: {error}"))?;
    socket
        .send(request)
        .await
        .map_err(|error| format!("cannot send to {server}: {error}"))?;
    let mut buffer = vec![0_u8; 4096];
    let length = socket
        .recv(&mut buffer)
        .await
        .map_err(|error| format!("cannot receive from {server}: {error}"))?;
    buffer.truncate(length);
    Ok(buffer)
}

async fn tcp_exchange(
    request: &[u8],
    server: SocketAddr,
    interface: &Interface,
) -> Result<Vec<u8>, String> {
    let socket = if server.is_ipv6() {
        tokio::net::TcpSocket::new_v6()
    } else {
        tokio::net::TcpSocket::new_v4()
    }
    .map_err(|error| format!("cannot create a DNS TCP socket: {error}"))?;
    pin_socket(
        &socket2::SockRef::from(&socket),
        interface,
        server.is_ipv6(),
    )?;
    let mut stream = socket
        .connect(server)
        .await
        .map_err(|error| format!("cannot connect to {server} over TCP: {error}"))?;
    let length =
        u16::try_from(request.len()).map_err(|_| "the DNS query is too long".to_string())?;
    stream
        .write_all(&length.to_be_bytes())
        .await
        .map_err(|error| error.to_string())?;
    stream
        .write_all(request)
        .await
        .map_err(|error| error.to_string())?;
    let mut length = [0_u8; 2];
    stream
        .read_exact(&mut length)
        .await
        .map_err(|error| error.to_string())?;
    let mut response = vec![0_u8; u16::from_be_bytes(length) as usize];
    stream
        .read_exact(&mut response)
        .await
        .map_err(|error| error.to_string())?;
    Ok(response)
}

fn build_query(id: u16, host: &str, record_type: u16) -> Result<Vec<u8>, String> {
    let mut packet = Vec::with_capacity(64);
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&0x0100_u16.to_be_bytes());
    packet.extend_from_slice(&1_u16.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    for label in host.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(format!("\"{host}\" is not a valid DNS name."));
        }
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0);
    packet.extend_from_slice(&record_type.to_be_bytes());
    packet.extend_from_slice(&CLASS_IN.to_be_bytes());
    Ok(packet)
}

struct Parsed {
    addresses: Vec<IpAddr>,
    truncated: bool,
}

fn parse_response(packet: &[u8], expected_id: u16) -> Result<Parsed, String> {
    if packet.len() < 12 {
        return Err("the DNS answer is too short".to_string());
    }
    let id = u16::from_be_bytes([packet[0], packet[1]]);
    if id != expected_id {
        return Err("the DNS answer carries another query's id".to_string());
    }
    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    let truncated = flags & 0x0200 != 0;
    let rcode = flags & 0x000f;
    if rcode != 0 {
        return Err(match rcode {
            3 => "the name does not exist (NXDOMAIN)".to_string(),
            2 => "the resolver reported a server failure".to_string(),
            5 => "the resolver refused the query".to_string(),
            other => format!("the resolver answered rcode {other}"),
        });
    }
    let questions = u16::from_be_bytes([packet[4], packet[5]]) as usize;
    let answers = u16::from_be_bytes([packet[6], packet[7]]) as usize;
    let mut offset = 12;
    for _ in 0..questions {
        offset = skip_name(packet, offset)?;
        offset += 4;
    }
    let mut addresses = Vec::new();
    for _ in 0..answers {
        offset = skip_name(packet, offset)?;
        if offset + 10 > packet.len() {
            return Err("a DNS answer record is truncated".to_string());
        }
        let record_type = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
        let data_length = u16::from_be_bytes([packet[offset + 8], packet[offset + 9]]) as usize;
        offset += 10;
        if offset + data_length > packet.len() {
            return Err("a DNS answer record is truncated".to_string());
        }
        let data = &packet[offset..offset + data_length];
        match record_type {
            TYPE_A if data_length == 4 => {
                addresses.push(IpAddr::V4(Ipv4Addr::new(
                    data[0], data[1], data[2], data[3],
                )));
            }
            TYPE_AAAA if data_length == 16 => {
                let mut bytes = [0_u8; 16];
                bytes.copy_from_slice(data);
                addresses.push(IpAddr::V6(Ipv6Addr::from(bytes)));
            }
            TYPE_CNAME | _ => {}
        }
        offset += data_length;
    }
    Ok(Parsed {
        addresses,
        truncated,
    })
}

fn skip_name(packet: &[u8], mut offset: usize) -> Result<usize, String> {
    loop {
        let Some(&length) = packet.get(offset) else {
            return Err("a DNS name runs past the packet".to_string());
        };
        if length & 0xc0 == 0xc0 {
            return Ok(offset + 2);
        }
        if length == 0 {
            return Ok(offset + 1);
        }
        offset += 1 + length as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::{build_query, parse_response};

    #[test]
    fn answers_parse_and_compressed_names_are_skipped() {
        let query = build_query(0x1234, "hub.example", super::TYPE_A).unwrap();
        assert_eq!(&query[12..], b"\x03hub\x07example\x00\x00\x01\x00\x01");
        let mut response = query.clone();
        response[2] = 0x81;
        response[3] = 0x80;
        response[7] = 2;
        // Two A records with a compressed owner name pointing at offset 12.
        for octets in [[203_u8, 0, 113, 5], [198, 18, 0, 1]] {
            response.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
            response.extend_from_slice(&octets);
        }
        let parsed = parse_response(&response, 0x1234).unwrap();
        assert_eq!(parsed.addresses.len(), 2);
        assert!(!parsed.truncated);
        assert!(parse_response(&response, 0x9999).is_err());
    }
}
