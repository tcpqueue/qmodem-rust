use crate::config::Server;
use anyhow::{Context, Result, ensure};
use nix::{ifaddrs::getifaddrs, net::if_::if_nametoindex};
use serde::Serialize;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
};

#[derive(Debug, Serialize)]
pub struct Interface {
    pub name: String,
    pub index: u32,
    pub addresses: Vec<IpAddr>,
}

pub fn interfaces() -> Result<Vec<Interface>> {
    let mut found: BTreeMap<String, Interface> = BTreeMap::new();
    for item in getifaddrs()? {
        let entry = found
            .entry(item.interface_name.clone())
            .or_insert_with(|| Interface {
                index: if_nametoindex(item.interface_name.as_str()).unwrap_or(0),
                name: item.interface_name,
                addresses: Vec::new(),
            });
        let address = item.address.and_then(|a| {
            a.as_sockaddr_in()
                .map(|s| IpAddr::V4(s.ip()))
                .or_else(|| a.as_sockaddr_in6().map(|s| IpAddr::V6(s.ip())))
        });
        if let Some(ip) = address
            && !entry.addresses.contains(&ip)
        {
            entry.addresses.push(ip);
        }
    }
    Ok(found.into_values().collect())
}

pub fn bind(server: &Server) -> Result<tokio::net::TcpListener> {
    let mut address = SocketAddr::new(server.listen, server.port);
    let socket = Socket::new(
        if server.listen.is_ipv6() {
            Domain::IPV6
        } else {
            Domain::IPV4
        },
        Type::STREAM,
        Some(Protocol::TCP),
    )?;
    socket.set_reuse_address(true)?;
    // IPv6 wildcard must not silently accept IPv4 on an unrequested interface.
    if server.listen.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    if !server.interface.is_empty() {
        let device = interfaces()?
            .into_iter()
            .find(|i| i.name == server.interface)
            .with_context(|| format!("network device '{}' does not exist", server.interface))?;
        ensure!(
            server.listen.is_unspecified() || device.addresses.contains(&server.listen),
            "listen address {} is not assigned to network device {}",
            server.listen,
            server.interface
        );
        if let SocketAddr::V6(ref mut v6) = address
            && v6.ip().is_unicast_link_local()
        {
            v6.set_scope_id(device.index);
        }
        socket
            .bind_device(Some(server.interface.as_bytes()))
            .with_context(|| {
                format!(
                    "bind to network device {}; SO_BINDTODEVICE may require CAP_NET_RAW",
                    server.interface
                )
            })?;
    } else if let IpAddr::V6(ip) = server.listen {
        ensure!(
            !ip.is_unicast_link_local(),
            "an IPv6 link-local listen address requires server.interface"
        );
    }
    socket.set_nonblocking(true)?;
    socket
        .bind(&address.into())
        .with_context(|| format!("bind HTTP listener {address}"))?;
    socket.listen(128)?;
    tokio::net::TcpListener::from_std(socket.into()).context("register HTTP listener")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovers_loopback_without_shell() {
        let all = interfaces().unwrap();
        let lo = all.iter().find(|i| i.name == "lo").unwrap();
        assert!(lo.addresses.iter().any(IpAddr::is_loopback));
    }
    #[tokio::test]
    async fn nonexistent_device_fails_instead_of_listening_everywhere() {
        let cfg = Server {
            listen: "0.0.0.0".parse().unwrap(),
            port: 8088,
            interface: "qmr-missing".into(),
        };
        assert!(
            bind(&cfg)
                .unwrap_err()
                .to_string()
                .contains("does not exist")
        );
    }
    #[tokio::test]
    async fn loopback_listener_is_reachable() {
        let cfg = Server {
            listen: "127.0.0.1".parse().unwrap(),
            port: 0,
            interface: String::new(),
        };
        let socket = bind(&cfg).unwrap();
        tokio::net::TcpStream::connect(socket.local_addr().unwrap())
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn wrong_address_on_device_is_rejected() {
        let cfg = Server {
            listen: "192.0.2.55".parse().unwrap(),
            port: 8088,
            interface: "lo".into(),
        };
        assert!(bind(&cfg).unwrap_err().to_string().contains("not assigned"));
    }
}
