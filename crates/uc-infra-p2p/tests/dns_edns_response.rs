use std::{
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};

use iroh::dns::DnsResolver;
use tokio::{net::UdpSocket, time::timeout};

// 公开中继域名的捕获响应；A 记录是代理保留网段中的虚拟地址。
const RESPONSE: &str = "12-34-85-80-00-01-00-01-00-00-00-01-06-75-73-65-31-2D-31-05-72-65-6C-61-79-02-6E-30-04-69-72-6F-68-04-6C-69-6E-6B-00-00-01-00-01-C0-0C-00-01-00-01-00-00-00-01-00-04-C6-12-00-19-00-00-29-04-D0-00-00-00-01-00-00";

#[tokio::test]
async fn relay_lookup_accepts_edns_reserved_flags_and_dnssec_do() -> anyhow::Result<()> {
    for flags in [1_u32, 0x100, 0x8000, 0xffff] {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let resolver = DnsResolver::with_nameserver(socket.local_addr()?);
        let serve = async {
            let mut query = [0_u8; 4096];
            let (len, peer) =
                timeout(Duration::from_secs(2), socket.recv_from(&mut query)).await??;
            assert!(len >= 12);
            let mut response = RESPONSE
                .split('-')
                .map(|part| u8::from_str_radix(part, 16))
                .collect::<Result<Vec<_>, _>>()?;
            response[..2].copy_from_slice(&query[..2]);
            let ttl = response.len() - 6;
            response[ttl..ttl + 4].copy_from_slice(&flags.to_be_bytes());
            socket.send_to(&response, peer).await?;
            Ok::<_, anyhow::Error>(())
        };
        let (result, served) = tokio::join!(
            resolver.lookup_ipv4("use1-1.relay.n0.iroh.link.", Duration::from_secs(2)),
            serve,
        );
        served?;
        assert_eq!(
            result?.collect::<Vec<_>>(),
            vec![IpAddr::V4(Ipv4Addr::new(198, 18, 0, 25))]
        );
    }
    Ok(())
}
