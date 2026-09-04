//! Where the hub link leaves this machine: physical interfaces, their DNS servers and
//! gateways, and the socket options that pin a connection to one of them so that a TUN
//! adapter's default route or a system proxy never carries World traffic.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// One network interface as the link layer sees it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Interface {
    pub(crate) name: String,
    pub(crate) index: u32,
    pub(crate) ipv4: Vec<Ipv4Addr>,
    pub(crate) ipv6: Vec<Ipv6Addr>,
    /// DNS servers configured on this interface, fake-IP ranges already removed.
    pub(crate) dns: Vec<IpAddr>,
    pub(crate) gateway_v4: Option<Ipv4Addr>,
    pub(crate) gateway_v6: Option<Ipv6Addr>,
    /// Route metric; lower wins.
    pub(crate) metric: u32,
    /// Ethernet or Wi-Fi hardware, up, with a usable address.
    pub(crate) physical: bool,
    /// Looks like a TUN, TAP, VPN, or virtual-switch adapter that is up with an address.
    pub(crate) tunnel: bool,
}

impl Interface {
    /// The address a pinned socket binds to for the given family.
    pub(crate) fn local_address(&self, ipv6: bool) -> Option<IpAddr> {
        if ipv6 {
            self.ipv6.first().copied().map(IpAddr::V6)
        } else {
            self.ipv4.first().copied().map(IpAddr::V4)
        }
    }

    /// DNS servers, falling back to the gateway when the interface names none.
    pub(crate) fn resolvers(&self) -> Vec<IpAddr> {
        if !self.dns.is_empty() {
            return self.dns.clone();
        }
        self.gateway_v4
            .map(IpAddr::V4)
            .into_iter()
            .chain(self.gateway_v6.map(IpAddr::V6))
            .collect()
    }
}

/// Everything the scan found.
#[derive(Clone, Debug, Default)]
pub(crate) struct NetworkView {
    pub(crate) interfaces: Vec<Interface>,
}

impl NetworkView {
    /// Names of tunnel adapters that are up: the things `direct` mode bypasses.
    pub(crate) fn tunnels(&self) -> Vec<String> {
        self.interfaces
            .iter()
            .filter(|interface| interface.tunnel)
            .map(|interface| interface.name.clone())
            .collect()
    }

    /// The physical interface to pin to: the named one when given, otherwise the one that
    /// owns a default gateway with the lowest metric, otherwise any physical interface.
    pub(crate) fn choose_physical(&self, preferred: Option<&str>) -> Result<&Interface, String> {
        if let Some(name) = preferred {
            return self
                .interfaces
                .iter()
                .find(|interface| interface.name == name)
                .filter(|interface| !interface.ipv4.is_empty() || !interface.ipv6.is_empty())
                .ok_or_else(|| {
                    format!(
                        "no_physical_interface: the configured interface \"{name}\" is not up with an address."
                    )
                });
        }
        let mut candidates = self
            .interfaces
            .iter()
            .filter(|interface| interface.physical)
            .collect::<Vec<_>>();
        candidates.sort_by_key(|interface| {
            (
                interface.gateway_v4.is_none() && interface.gateway_v6.is_none(),
                interface.metric,
                interface.name.clone(),
            )
        });
        candidates.first().copied().ok_or_else(|| {
            "no_physical_interface: no Ethernet or Wi-Fi interface is up with an address."
                .to_string()
        })
    }

    /// A hash of interface names and addresses; a change means the link should reselect.
    pub(crate) fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        for interface in &self.interfaces {
            interface.name.hash(&mut hasher);
            interface.ipv4.hash(&mut hasher);
            interface.ipv6.hash(&mut hasher);
            interface.gateway_v4.hash(&mut hasher);
            interface.physical.hash(&mut hasher);
        }
        hasher.finish()
    }
}

/// Fake-IP ranges handed out by TUN proxies in place of real answers.
pub(crate) fn is_fake_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19)) || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            // fc00::/18
            (segments[0] & 0xffc0) == 0xfc00 || v6.is_unspecified()
        }
    }
}

fn usable_v4(address: Ipv4Addr) -> bool {
    !address.is_loopback() && !address.is_link_local() && !address.is_unspecified()
}

fn usable_v6(address: Ipv6Addr) -> bool {
    !address.is_loopback()
        && !address.is_unspecified()
        && (address.segments()[0] & 0xffc0) != 0xfe80
}

fn tunnel_like_name(name: &str) -> bool {
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
        "sing",
        "surge",
        "proxy",
        "lo",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

/// Scans the machine's interfaces.
pub(crate) fn scan() -> Result<NetworkView, String> {
    platform::scan()
}

/// Pins `socket` to `interface` so its packets leave through it regardless of the routing
/// table, and binds it to that interface's address.
pub(crate) fn pin_socket(
    socket: &socket2::SockRef<'_>,
    interface: &Interface,
    ipv6: bool,
) -> Result<(), String> {
    platform::pin_socket(socket, interface, ipv6)
}

#[cfg(windows)]
mod platform {
    use super::{Interface, NetworkView, is_fake_ip, tunnel_like_name, usable_v4, usable_v6};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST,
        GetAdaptersAddresses, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{
        AF_INET, AF_INET6, AF_UNSPEC, IP_UNICAST_IF, IPPROTO_IP, IPPROTO_IPV6, IPV6_UNICAST_IF,
        SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6, SOCKET, setsockopt,
    };

    const IF_TYPE_PROP_VIRTUAL: u32 = 53;
    const IF_TYPE_TUNNEL: u32 = 131;

    pub(super) fn scan() -> Result<NetworkView, String> {
        let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
        let mut size: u32 = 16 * 1024;
        let mut buffer: Vec<u8>;
        loop {
            buffer = vec![0_u8; size as usize];
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
                0 => break,
                code if code == ERROR_BUFFER_OVERFLOW => continue,
                code => {
                    return Err(format!(
                        "GetAdaptersAddresses failed with Windows error {code}."
                    ));
                }
            }
        }
        let mut interfaces = Vec::new();
        let mut cursor = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !cursor.is_null() {
            let adapter = unsafe { &*cursor };
            let name = wide_string(adapter.FriendlyName);
            let description = wide_string(adapter.Description);
            let index = unsafe { adapter.Anonymous1.Anonymous.IfIndex };
            let up = adapter.OperStatus == IfOperStatusUp;
            let mut ipv4 = Vec::new();
            let mut ipv6 = Vec::new();
            let mut unicast = adapter.FirstUnicastAddress;
            while !unicast.is_null() {
                let entry = unsafe { &*unicast };
                match socket_address(entry.Address.lpSockaddr) {
                    Some(IpAddr::V4(address)) if usable_v4(address) => ipv4.push(address),
                    Some(IpAddr::V6(address)) if usable_v6(address) => ipv6.push(address),
                    _ => {}
                }
                unicast = entry.Next;
            }
            let mut dns = Vec::new();
            let mut server = adapter.FirstDnsServerAddress;
            while !server.is_null() {
                let entry = unsafe { &*server };
                if let Some(address) = socket_address(entry.Address.lpSockaddr)
                    && !is_fake_ip(address)
                    && !address.is_loopback()
                {
                    dns.push(address);
                }
                server = entry.Next;
            }
            let mut gateway_v4 = None;
            let mut gateway_v6 = None;
            let mut gateway = adapter.FirstGatewayAddress;
            while !gateway.is_null() {
                let entry = unsafe { &*gateway };
                match socket_address(entry.Address.lpSockaddr) {
                    Some(IpAddr::V4(address)) if gateway_v4.is_none() => gateway_v4 = Some(address),
                    Some(IpAddr::V6(address)) if gateway_v6.is_none() => gateway_v6 = Some(address),
                    _ => {}
                }
                gateway = entry.Next;
            }
            let has_address = !ipv4.is_empty() || !ipv6.is_empty();
            let hardware = matches!(adapter.IfType, IF_TYPE_ETHERNET_CSMACD | IF_TYPE_IEEE80211);
            let virtual_by_type = matches!(adapter.IfType, IF_TYPE_PROP_VIRTUAL | IF_TYPE_TUNNEL);
            let virtual_by_name = tunnel_like_name(&name)
                || [
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
                ]
                .iter()
                .any(|needle| description.to_ascii_lowercase().contains(needle));
            let tunnel = up && has_address && (virtual_by_type || virtual_by_name);
            let physical = up
                && has_address
                && hardware
                && !virtual_by_name
                && adapter.IfType != IF_TYPE_PROP_VIRTUAL;
            interfaces.push(Interface {
                name: if name.is_empty() {
                    description.clone()
                } else {
                    name
                },
                index,
                ipv4,
                ipv6,
                dns,
                gateway_v4,
                gateway_v6,
                metric: adapter.Ipv4Metric.min(adapter.Ipv6Metric.max(1)),
                physical,
                tunnel,
            });
            cursor = adapter.Next;
        }
        Ok(NetworkView { interfaces })
    }

    fn wide_string(pointer: *const u16) -> String {
        if pointer.is_null() {
            return String::new();
        }
        let mut length = 0;
        while unsafe { *pointer.add(length) } != 0 {
            length += 1;
        }
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(pointer, length) })
    }

    fn socket_address(pointer: *const SOCKADDR) -> Option<IpAddr> {
        if pointer.is_null() {
            return None;
        }
        let family = unsafe { (*pointer).sa_family };
        if family == AF_INET {
            let address = unsafe { &*pointer.cast::<SOCKADDR_IN>() };
            let raw = unsafe { address.sin_addr.S_un.S_addr };
            Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(raw))))
        } else if family == AF_INET6 {
            let address = unsafe { &*pointer.cast::<SOCKADDR_IN6>() };
            let bytes = unsafe { address.sin6_addr.u.Byte };
            Some(IpAddr::V6(Ipv6Addr::from(bytes)))
        } else {
            None
        }
    }

    pub(super) fn pin_socket(
        socket: &socket2::SockRef<'_>,
        interface: &Interface,
        ipv6: bool,
    ) -> Result<(), String> {
        let raw = socket.as_raw_socket() as SOCKET;
        // IP_UNICAST_IF takes the index in network byte order for IPv4 and host order for IPv6.
        let (level, option, value) = if ipv6 {
            (IPPROTO_IPV6, IPV6_UNICAST_IF, interface.index)
        } else {
            (IPPROTO_IP, IP_UNICAST_IF, interface.index.to_be())
        };
        let result = unsafe {
            setsockopt(
                raw,
                level,
                option,
                (&value as *const u32).cast::<u8>(),
                std::mem::size_of::<u32>() as i32,
            )
        };
        if result != 0 {
            return Err(format!(
                "cannot pin the socket to \"{}\": {}",
                interface.name,
                std::io::Error::last_os_error()
            ));
        }
        let local = interface.local_address(ipv6).ok_or_else(|| {
            format!(
                "\"{}\" has no {} address.",
                interface.name,
                if ipv6 { "IPv6" } else { "IPv4" }
            )
        })?;
        socket
            .bind(&std::net::SocketAddr::new(local, 0).into())
            .map_err(|error| format!("cannot bind to {local} on \"{}\": {error}", interface.name))
    }
}

#[cfg(unix)]
mod platform {
    use super::{Interface, NetworkView, is_fake_ip, tunnel_like_name, usable_v4, usable_v6};
    use std::collections::BTreeMap;
    use std::ffi::CStr;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    pub(super) fn scan() -> Result<NetworkView, String> {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        if unsafe { libc::getifaddrs(&mut head) } != 0 {
            return Err(format!(
                "getifaddrs failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut by_name: BTreeMap<String, Interface> = BTreeMap::new();
        let mut cursor = head;
        while !cursor.is_null() {
            let entry = unsafe { &*cursor };
            cursor = entry.ifa_next;
            if entry.ifa_name.is_null() || entry.ifa_addr.is_null() {
                continue;
            }
            let name = unsafe { CStr::from_ptr(entry.ifa_name) }
                .to_string_lossy()
                .into_owned();
            let flags = entry.ifa_flags as libc::c_int;
            let up = flags & libc::IFF_UP != 0 && flags & libc::IFF_RUNNING != 0;
            let loopback = flags & libc::IFF_LOOPBACK != 0;
            let interface = by_name.entry(name.clone()).or_insert_with(|| Interface {
                index: unsafe { libc::if_nametoindex(entry.ifa_name) },
                name: name.clone(),
                ipv4: Vec::new(),
                ipv6: Vec::new(),
                dns: Vec::new(),
                gateway_v4: None,
                gateway_v6: None,
                metric: 0,
                physical: false,
                tunnel: false,
            });
            if !up || loopback {
                continue;
            }
            let family = unsafe { (*entry.ifa_addr).sa_family } as libc::c_int;
            if family == libc::AF_INET {
                let address = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in>() };
                let v4 = Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr));
                if usable_v4(v4) {
                    interface.ipv4.push(v4);
                }
            } else if family == libc::AF_INET6 {
                let address = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in6>() };
                let v6 = Ipv6Addr::from(address.sin6_addr.s6_addr);
                if usable_v6(v6) {
                    interface.ipv6.push(v6);
                }
            }
        }
        unsafe { libc::freeifaddrs(head) };

        let (default_interface, gateway_v4, metric) = default_route();
        let dns = system_dns();
        let mut interfaces = Vec::new();
        for (name, mut interface) in by_name {
            let has_address = !interface.ipv4.is_empty() || !interface.ipv6.is_empty();
            let hardware = hardware_like(&name);
            let tunnel_name = tunnel_like_name(&name);
            interface.tunnel = has_address && tunnel_name;
            interface.physical = has_address && hardware && !tunnel_name;
            if default_interface.as_deref() == Some(name.as_str()) {
                interface.gateway_v4 = gateway_v4;
                interface.metric = metric;
            } else {
                interface.metric = 1000;
            }
            if interface.physical {
                interface.dns = interface_dns(&name)
                    .unwrap_or_else(|| dns.clone())
                    .into_iter()
                    .filter(|address| !is_fake_ip(*address) && !address.is_loopback())
                    .collect();
            }
            interfaces.push(interface);
        }
        Ok(NetworkView { interfaces })
    }

    fn hardware_like(name: &str) -> bool {
        if cfg!(target_os = "linux") {
            // A real NIC has a device node; tunnels, bridges, and veth pairs do not.
            let device = std::path::Path::new("/sys/class/net")
                .join(name)
                .join("device");
            if device.exists() {
                return true;
            }
        }
        ["en", "eth", "wl", "ww", "usb"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
            && !["bridge", "awdl", "llw", "ap", "utun", "vmnet"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
    }

    /// The default route's interface, gateway, and metric.
    fn default_route() -> (Option<String>, Option<Ipv4Addr>, u32) {
        if cfg!(target_os = "linux") {
            if let Ok(table) = std::fs::read_to_string("/proc/net/route") {
                let mut best: Option<(String, Ipv4Addr, u32)> = None;
                for line in table.lines().skip(1) {
                    let fields = line.split_whitespace().collect::<Vec<_>>();
                    if fields.len() < 8 || fields[1] != "00000000" {
                        continue;
                    }
                    let gateway = u32::from_str_radix(fields[2], 16)
                        .ok()
                        .map(|raw| Ipv4Addr::from(u32::from_be(raw)));
                    let metric = fields[6].parse::<u32>().unwrap_or(0);
                    let Some(gateway) = gateway else {
                        continue;
                    };
                    if best
                        .as_ref()
                        .is_none_or(|(_, _, current)| metric < *current)
                    {
                        best = Some((fields[0].to_string(), gateway, metric));
                    }
                }
                if let Some((name, gateway, metric)) = best {
                    return (Some(name), Some(gateway), metric);
                }
            }
            return (None, None, 0);
        }
        let output = std::process::Command::new("route")
            .args(["-n", "get", "default"])
            .output();
        let Ok(output) = output else {
            return (None, None, 0);
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let mut name = None;
        let mut gateway = None;
        for line in text.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("interface:") {
                name = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("gateway:") {
                gateway = value.trim().parse::<Ipv4Addr>().ok();
            }
        }
        (name, gateway, 0)
    }

    /// Per-interface DNS where the platform exposes it (macOS DHCP option, systemd-resolved).
    fn interface_dns(name: &str) -> Option<Vec<IpAddr>> {
        if cfg!(target_os = "macos") {
            let output = std::process::Command::new("ipconfig")
                .args(["getoption", name, "domain_name_server"])
                .output()
                .ok()?;
            let servers = String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .filter_map(|token| token.parse::<IpAddr>().ok())
                .collect::<Vec<_>>();
            return (!servers.is_empty()).then_some(servers);
        }
        if cfg!(target_os = "linux") {
            let output = std::process::Command::new("resolvectl")
                .args(["dns", name])
                .output()
                .ok()?;
            let servers = String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .filter_map(|token| token.parse::<IpAddr>().ok())
                .collect::<Vec<_>>();
            return (!servers.is_empty()).then_some(servers);
        }
        None
    }

    /// Resolvers from the system files, preferring systemd-resolved's upstream list over its
    /// loopback stub.
    fn system_dns() -> Vec<IpAddr> {
        for path in ["/run/systemd/resolve/resolv.conf", "/etc/resolv.conf"] {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            let servers = text
                .lines()
                .filter_map(|line| line.trim().strip_prefix("nameserver"))
                .filter_map(|rest| rest.trim().parse::<IpAddr>().ok())
                .filter(|address| !address.is_loopback())
                .collect::<Vec<_>>();
            if !servers.is_empty() {
                return servers;
            }
        }
        Vec::new()
    }

    pub(super) fn pin_socket(
        socket: &socket2::SockRef<'_>,
        interface: &Interface,
        ipv6: bool,
    ) -> Result<(), String> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            // SO_BINDTODEVICE needs no privilege since Linux 5.7; older kernels fall back to
            // the address bind below, which is honest but weaker.
            let _ = socket.bind_device(Some(interface.name.as_bytes()));
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            let index = std::num::NonZeroU32::new(interface.index);
            let bound = if ipv6 {
                socket.bind_device_by_index_v6(index)
            } else {
                socket.bind_device_by_index_v4(index)
            };
            bound.map_err(|error| {
                format!("cannot pin the socket to \"{}\": {error}", interface.name)
            })?;
        }
        let local = interface.local_address(ipv6).ok_or_else(|| {
            format!(
                "\"{}\" has no {} address.",
                interface.name,
                if ipv6 { "IPv6" } else { "IPv4" }
            )
        })?;
        socket
            .bind(&std::net::SocketAddr::new(local, 0).into())
            .map_err(|error| format!("cannot bind to {local} on \"{}\": {error}", interface.name))
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use super::{Interface, NetworkView};

    pub(super) fn scan() -> Result<NetworkView, String> {
        Err(
            "no_physical_interface: interface enumeration is not supported on this platform."
                .to_string(),
        )
    }

    pub(super) fn pin_socket(
        _socket: &socket2::SockRef<'_>,
        _interface: &Interface,
        _ipv6: bool,
    ) -> Result<(), String> {
        Err("interface pinning is not supported on this platform.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{Interface, NetworkView, is_fake_ip};
    use std::net::{IpAddr, Ipv4Addr};

    fn interface(name: &str, gateway: bool, metric: u32, physical: bool) -> Interface {
        Interface {
            name: name.to_string(),
            index: 1,
            ipv4: vec![Ipv4Addr::new(192, 168, 1, 5)],
            ipv6: Vec::new(),
            dns: Vec::new(),
            gateway_v4: gateway.then_some(Ipv4Addr::new(192, 168, 1, 1)),
            gateway_v6: None,
            metric,
            physical,
            tunnel: !physical,
        }
    }

    #[test]
    fn the_gateway_owner_with_the_lowest_metric_wins_and_tunnels_never_do() {
        let view = NetworkView {
            interfaces: vec![
                interface("Meta", true, 0, false),
                interface("Ethernet 2", true, 25, true),
                interface("Wi-Fi", true, 35, true),
                interface("vEthernet (WSL)", false, 5, true),
            ],
        };
        assert_eq!(view.choose_physical(None).unwrap().name, "Ethernet 2");
        assert_eq!(view.choose_physical(Some("Wi-Fi")).unwrap().name, "Wi-Fi");
        assert_eq!(view.tunnels(), vec!["Meta"]);
        assert!(
            view.choose_physical(Some("nope"))
                .unwrap_err()
                .contains("no_physical_interface")
        );
        assert!(is_fake_ip(IpAddr::V4(Ipv4Addr::new(198, 18, 0, 5))));
        assert!(is_fake_ip("fc00::1".parse().unwrap()));
        assert!(!is_fake_ip(IpAddr::V4(Ipv4Addr::new(121, 40, 82, 28))));
    }
}
