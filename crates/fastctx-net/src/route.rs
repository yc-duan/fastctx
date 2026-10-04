//! Which way a connection to the hub leaves this machine.
//!
//! Proxies such as Clash or FlClash often run a TUN adapter that takes the
//! default route; if the World's connection went through it, the proxy
//! stalling or restarting would take the World down with it. So a
//! connection is first pinned to the physical network card, past any TUN,
//! and only when that fails does it go the way the system sends things:
//! through the system's proxy, or along its routes (`world-network.md`).
//! The hub is always reached by its IP address, so no name is looked up.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// A network interface as this module sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub index: u32,
    pub ipv4: Vec<Ipv4Addr>,
    pub ipv6: Vec<Ipv6Addr>,
    /// Whether the interface has a default gateway.
    pub gateway: bool,
    /// The routing metric; lower wins.
    pub metric: u32,
    /// Ethernet or Wi-Fi hardware that is up with a usable address, and not
    /// a TUN, TAP, VPN or virtual switch.
    pub physical: bool,
}

impl Interface {
    fn address(&self, ipv6: bool) -> Option<IpAddr> {
        if ipv6 { self.ipv6.first().copied().map(IpAddr::V6) } else { self.ipv4.first().copied().map(IpAddr::V4) }
    }
}

/// The physical interface to leave through: one with a default gateway
/// before one without, then the lowest metric.
pub fn physical(interfaces: &[Interface]) -> Option<&Interface> {
    interfaces
        .iter()
        .filter(|interface| interface.physical)
        .min_by_key(|interface| (!interface.gateway, interface.metric, interface.name.clone()))
}

/// A TCP socket for `target`, pinned to `interface`: bound to its address
/// and, where the platform allows, to the device itself, so its packets
/// leave through it whatever the routing table says.
pub fn pinned_socket(interface: &Interface, target: SocketAddr) -> Result<tokio::net::TcpSocket, String> {
    let ipv6 = target.is_ipv6();
    let local = interface
        .address(ipv6)
        .ok_or_else(|| format!("\"{}\" has no {} address", interface.name, if ipv6 { "IPv6" } else { "IPv4" }))?;
    let socket = if ipv6 { tokio::net::TcpSocket::new_v6() } else { tokio::net::TcpSocket::new_v4() }
        .map_err(|error| format!("making a socket: {error}"))?;
    platform::pin(&socket2::SockRef::from(&socket), interface, ipv6)?;
    socket
        .bind(SocketAddr::new(local, 0))
        .map_err(|error| format!("binding to {local} on \"{}\": {error}", interface.name))?;
    Ok(socket)
}

/// Every interface that is up.
pub fn interfaces() -> Result<Vec<Interface>, String> {
    platform::interfaces()
}

fn usable_v4(address: Ipv4Addr) -> bool {
    !address.is_loopback() && !address.is_link_local() && !address.is_unspecified()
}

fn usable_v6(address: Ipv6Addr) -> bool {
    !address.is_loopback() && !address.is_unspecified() && (address.segments()[0] & 0xffc0) != 0xfe80
}

/// Names that TUN, VPN and virtual adapters go by.
fn virtual_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "tun",
        "utun",
        "tap",
        "wg",
        "wireguard",
        "tailscale",
        "zt",
        "docker",
        "veth",
        "br-",
        "virbr",
        "vmnet",
        "vboxnet",
        "hyper-v",
        "vethernet",
        "meta",
        "mihomo",
        "clash",
        "flclash",
        "sing",
        "surge",
        "proxy",
        "lo",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

#[cfg(windows)]
mod platform {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::os::windows::io::AsRawSocket as _;

    use windows_sys::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
        GetAdaptersAddresses, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{
        AF_INET, AF_INET6, AF_UNSPEC, IP_UNICAST_IF, IPPROTO_IP, IPPROTO_IPV6, IPV6_UNICAST_IF, SOCKADDR, SOCKADDR_IN,
        SOCKADDR_IN6, SOCKET, setsockopt,
    };

    use super::{Interface, usable_v4, usable_v6, virtual_name};

    const IF_TYPE_PROP_VIRTUAL: u32 = 53;
    const IF_TYPE_TUNNEL: u32 = 131;

    /// What adapters' descriptions say when they are virtual, whatever
    /// their names.
    const VIRTUAL_DESCRIPTIONS: [&str; 17] = [
        "wintun",
        "tap-windows",
        "tap ",
        "tun",
        "wireguard",
        "hyper-v",
        "vmware",
        "virtualbox",
        "openvpn",
        "zerotier",
        "tailscale",
        "clash",
        "mihomo",
        "meta",
        "sing-box",
        "surge",
        "proxifier",
    ];

    pub fn interfaces() -> Result<Vec<Interface>, String> {
        let flags =
            GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        // The size the call asks for when the buffer is too small; it is
        // asked again, since adapters may come and go in between.
        let mut size: u32 = 16 * 1024;
        let buffer = loop {
            let mut buffer = vec![0_u64; (size as usize).div_ceil(8)];
            // SAFETY: the buffer is `size` bytes and aligned for the structure.
            let result = unsafe {
                GetAdaptersAddresses(
                    u32::from(AF_UNSPEC),
                    flags,
                    std::ptr::null(),
                    buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                    &mut size,
                )
            };
            match result {
                0 => break buffer,
                code if code == ERROR_BUFFER_OVERFLOW => continue,
                code => return Err(format!("listing the network adapters failed with Windows error {code}")),
            }
        };
        let mut interfaces = Vec::new();
        let mut cursor = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !cursor.is_null() {
            // SAFETY: the list lives in `buffer`, linked by `Next`.
            let adapter = unsafe { &*cursor };
            cursor = adapter.Next;
            if adapter.OperStatus != IfOperStatusUp {
                continue;
            }
            let name = wide(adapter.FriendlyName);
            let description = wide(adapter.Description);
            let mut ipv4 = Vec::new();
            let mut ipv6 = Vec::new();
            let mut unicast = adapter.FirstUnicastAddress;
            while !unicast.is_null() {
                // SAFETY: as above.
                let entry = unsafe { &*unicast };
                match address(entry.Address.lpSockaddr) {
                    Some(IpAddr::V4(address)) if usable_v4(address) => ipv4.push(address),
                    Some(IpAddr::V6(address)) if usable_v6(address) => ipv6.push(address),
                    _ => {}
                }
                unicast = entry.Next;
            }
            let gateway = !adapter.FirstGatewayAddress.is_null();
            let hardware = matches!(adapter.IfType, IF_TYPE_ETHERNET_CSMACD | IF_TYPE_IEEE80211);
            let lower = description.to_ascii_lowercase();
            let virtual_adapter = matches!(adapter.IfType, IF_TYPE_PROP_VIRTUAL | IF_TYPE_TUNNEL)
                || virtual_name(&name)
                || VIRTUAL_DESCRIPTIONS.iter().any(|needle| lower.contains(needle));
            let has_address = !ipv4.is_empty() || !ipv6.is_empty();
            interfaces.push(Interface {
                // SAFETY: a union whose index member is always set.
                index: unsafe { adapter.Anonymous1.Anonymous.IfIndex },
                name: if name.is_empty() { description } else { name },
                ipv4,
                ipv6,
                gateway,
                metric: adapter.Ipv4Metric,
                physical: has_address && hardware && !virtual_adapter,
            });
        }
        Ok(interfaces)
    }

    fn wide(pointer: *const u16) -> String {
        if pointer.is_null() {
            return String::new();
        }
        let mut length = 0;
        // SAFETY: a NUL-terminated string from the adapter list.
        while unsafe { *pointer.add(length) } != 0 {
            length += 1;
        }
        // SAFETY: `length` units were just read.
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(pointer, length) })
    }

    fn address(pointer: *const SOCKADDR) -> Option<IpAddr> {
        if pointer.is_null() {
            return None;
        }
        // SAFETY: a socket address from the adapter list, read as its family says.
        unsafe {
            match (*pointer).sa_family {
                AF_INET => {
                    let raw = (*pointer.cast::<SOCKADDR_IN>()).sin_addr.S_un.S_addr;
                    Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(raw))))
                }
                AF_INET6 => Some(IpAddr::V6(Ipv6Addr::from((*pointer.cast::<SOCKADDR_IN6>()).sin6_addr.u.Byte))),
                _ => None,
            }
        }
    }

    pub fn pin(socket: &socket2::SockRef<'_>, interface: &Interface, ipv6: bool) -> Result<(), String> {
        // The interface index goes in network byte order for IPv4 and in
        // host byte order for IPv6.
        let (level, option, value) = if ipv6 {
            (IPPROTO_IPV6, IPV6_UNICAST_IF, interface.index)
        } else {
            (IPPROTO_IP, IP_UNICAST_IF, interface.index.to_be())
        };
        // SAFETY: a live socket and a u32 option value.
        let result = unsafe {
            setsockopt(
                socket.as_raw_socket() as SOCKET,
                level,
                option,
                (&raw const value).cast::<u8>(),
                size_of::<u32>() as i32,
            )
        };
        if result != 0 {
            return Err(format!("pinning to \"{}\": {}", interface.name, std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

#[cfg(unix)]
mod platform {
    use std::collections::BTreeMap;
    use std::ffi::CStr;
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::{Interface, usable_v4, usable_v6, virtual_name};

    pub fn interfaces() -> Result<Vec<Interface>, String> {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: getifaddrs fills `head` with a list freed below.
        if unsafe { libc::getifaddrs(&mut head) } != 0 {
            return Err(format!("listing the network interfaces: {}", std::io::Error::last_os_error()));
        }
        let mut by_name: BTreeMap<String, Interface> = BTreeMap::new();
        let mut cursor = head;
        while !cursor.is_null() {
            // SAFETY: an entry of the list getifaddrs made.
            let entry = unsafe { &*cursor };
            cursor = entry.ifa_next;
            if entry.ifa_name.is_null() || entry.ifa_addr.is_null() {
                continue;
            }
            let flags = entry.ifa_flags as libc::c_int;
            if flags & libc::IFF_UP == 0 || flags & libc::IFF_RUNNING == 0 || flags & libc::IFF_LOOPBACK != 0 {
                continue;
            }
            // SAFETY: a NUL-terminated name.
            let name = unsafe { CStr::from_ptr(entry.ifa_name) }.to_string_lossy().into_owned();
            let interface = by_name.entry(name.clone()).or_insert_with(|| Interface {
                // SAFETY: as above.
                index: unsafe { libc::if_nametoindex(entry.ifa_name) },
                name,
                ipv4: Vec::new(),
                ipv6: Vec::new(),
                gateway: false,
                metric: u32::MAX,
                physical: false,
            });
            // SAFETY: the address is read as its family says.
            unsafe {
                match (*entry.ifa_addr).sa_family as libc::c_int {
                    libc::AF_INET => {
                        let raw = (*entry.ifa_addr.cast::<libc::sockaddr_in>()).sin_addr.s_addr;
                        let address = Ipv4Addr::from(u32::from_be(raw));
                        if usable_v4(address) {
                            interface.ipv4.push(address);
                        }
                    }
                    libc::AF_INET6 => {
                        let address = Ipv6Addr::from((*entry.ifa_addr.cast::<libc::sockaddr_in6>()).sin6_addr.s6_addr);
                        if usable_v6(address) {
                            interface.ipv6.push(address);
                        }
                    }
                    _ => {}
                }
            }
        }
        // SAFETY: the list from getifaddrs, freed once.
        unsafe { libc::freeifaddrs(head) };
        let defaults = default_routes();
        Ok(by_name
            .into_values()
            .map(|mut interface| {
                if let Some(metric) = defaults.get(&interface.name) {
                    interface.gateway = true;
                    interface.metric = *metric;
                }
                let has_address = !interface.ipv4.is_empty() || !interface.ipv6.is_empty();
                interface.physical = has_address && hardware(&interface.name) && !virtual_name(&interface.name);
                interface
            })
            .collect())
    }

    /// A real network card has a device node; tunnels, bridges and veth
    /// pairs do not.
    fn hardware(name: &str) -> bool {
        std::path::Path::new("/sys/class/net").join(name).join("device").exists()
            || ["en", "eth", "wl", "ww", "usb"].iter().any(|prefix| name.starts_with(prefix))
    }

    /// The interfaces with an IPv4 default route, and the route's metric.
    fn default_routes() -> BTreeMap<String, u32> {
        let mut routes = BTreeMap::new();
        let Ok(table) = std::fs::read_to_string("/proc/net/route") else { return routes };
        for line in table.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 8 && fields[1] == "00000000" {
                let metric = fields[6].parse().unwrap_or(u32::MAX);
                let entry = routes.entry(fields[0].to_string()).or_insert(metric);
                *entry = (*entry).min(metric);
            }
        }
        routes
    }

    pub fn pin(socket: &socket2::SockRef<'_>, interface: &Interface, _ipv6: bool) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            // Binding to the device needs no privilege since Linux 5.7; on an
            // older kernel the bind to the interface's address still holds.
            let _ = socket.bind_device(Some(interface.name.as_bytes()));
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (socket, interface);
        Ok(())
    }
}
