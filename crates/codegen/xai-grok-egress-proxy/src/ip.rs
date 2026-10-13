use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    !(octets[0] == 0
        || ip.is_private()
        || (octets[0] == 100 && octets[1] & 0xc0 == 0x40)
        || ip.is_loopback()
        || ip.is_link_local()
        || (octets[0] == 192
            && octets[1] == 0
            && octets[2] == 0
            && octets[3] != 9
            && octets[3] != 10)
        || (octets[0] == 192 && octets[1] == 88 && octets[2] == 99)
        || ip.is_documentation()
        || (octets[0] == 198 && octets[1] & 0xfe == 18)
        || (octets[0] & 0xf0 == 0xf0 && !ip.is_broadcast())
        || ip.is_broadcast()
        || ip.is_multicast())
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(ipv4) = embedded_ipv4(ip) {
        return is_public_ipv4(ipv4);
    }
    let segments = ip.segments();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || matches!(segments, [0x64, 0xff9b, 1, _, _, _, _, _])
        || matches!(segments, [0x100, 0, 0, 0 | 1, _, _, _, _])
        || (segments[0] & 0xffc0 == 0xfec0)
        || (matches!(segments, [0x2001, b, _, _, _, _, _, _] if b < 0x200)
            && !(u128::from_be_bytes(ip.octets()) == 0x2001_0001_0000_0000_0000_0000_0000_0001
                || u128::from_be_bytes(ip.octets()) == 0x2001_0001_0000_0000_0000_0000_0000_0002
                || matches!(segments, [0x2001, 3, _, _, _, _, _, _])
                || matches!(segments, [0x2001, 4, 0x112, _, _, _, _, _])
                || matches!(segments, [0x2001, b, _, _, _, _, _, _] if (0x20..=0x3f).contains(&b))))
        || matches!(segments, [0x2002, _, _, _, _, _, _, _])
        || matches!(segments, [0x2001, 0xdb8, ..] | [0x3fff, 0..=0x0fff, ..])
        || matches!(segments, [0x5f00, ..])
        || ip.is_unique_local()
        || ip.is_unicast_link_local())
}

fn embedded_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = ip.segments();
    if matches!(segments, [0, 0, 0, 0, 0, 0 | 0xffff, _, _])
        || matches!(segments, [0x64, 0xff9b, 0, 0, 0, 0, _, _])
    {
        let octets = ip.octets();
        return Some(Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_private_reserved_and_embedded_addresses() {
        for value in [
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "169.254.169.254",
            "192.0.2.1",
            "192.88.99.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "::10.0.0.1",
            "::127.0.0.1",
            "::169.254.169.254",
            "::ffff:10.0.0.1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::10.0.0.1",
            "64:ff9b::127.0.0.1",
            "64:ff9b::169.254.169.254",
            "100:0:0:1::1",
            "fec0::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
        ] {
            assert!(!is_public_ip(value.parse().unwrap()), "{value}");
        }
        for value in ["8.8.8.8", "::8.8.8.8", "::ffff:8.8.8.8", "64:ff9b::8.8.8.8"] {
            assert!(is_public_ip(value.parse().unwrap()), "{value}");
        }
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }
}
