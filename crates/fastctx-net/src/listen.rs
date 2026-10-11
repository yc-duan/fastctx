//! Listening sockets that keep to their address family: one on an IPv6
//! address takes IPv6 connections only. A port can then be listened on for
//! both families at once the same way everywhere; Linux would otherwise let
//! `[::]` take IPv4 as well and refuse `0.0.0.0` the same port, where Windows
//! keeps them apart.

use std::net::SocketAddr;

use socket2::{Domain, Protocol, Socket, Type};

/// Listens on `address` as tokio's own `TcpListener::bind` does (address
/// reuse off Windows, a backlog of 1024), IPv6 kept to IPv6.
pub fn bind(address: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    let socket = Socket::new(Domain::for_address(address), Type::STREAM, Some(Protocol::TCP))?;
    if address.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    #[cfg(not(windows))]
    socket.set_reuse_address(true)?;
    socket.bind(&address.into())?;
    socket.listen(1024)?;
    socket.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(socket.into())
}
