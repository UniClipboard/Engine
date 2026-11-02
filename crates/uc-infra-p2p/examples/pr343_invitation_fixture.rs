//! 端到端网络验收的输入构造器。复用现有编解码器，只改变不可信候选地址。
//! 完整邀请只通过父进程管道传递，不写入文件或日志，也不建立网络连接。

use std::io::{self, Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use iroh::{EndpointAddr, TransportAddr};
use serde_json::{json, Value};
use uc_infra_p2p::network::iroh::space_admission::{
    decode_space_admission_continuation_endpoint, encode_space_admission_route,
};
use uc_sync_protocol::full_invitation::{decode_invitation_entry, encode_full_invitation};

fn main() -> Result<()> {
    let input: Value = serde_json::from_reader(io::stdin().lock().take(128 * 1024))?;
    let now_ms = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    let decoded = decode_invitation_entry(
        input["invitation"]
            .as_str()
            .context("missing full invitation")?,
        now_ms,
    )?
    .context("full invitation required")?;
    let original = decode_space_admission_continuation_endpoint(decoded.route())?;
    let mut addresses: Vec<_> = if input["replace"] == true {
        Vec::new()
    } else {
        original.addrs.iter().cloned().collect()
    };
    if let Some(direct) = input.get("direct") {
        for address in direct.as_array().context("direct address list required")? {
            addresses.push(TransportAddr::Ip(
                address.as_str().context("socket text required")?.parse()?,
            ));
        }
    }
    if let Some(relays) = input.get("relays") {
        for address in relays.as_array().context("relay list required")? {
            addresses.push(TransportAddr::Relay(
                address.as_str().context("relay text required")?.parse()?,
            ));
        }
    }
    let changed = EndpointAddr::from_parts(original.id, addresses);
    let route = encode_space_admission_route(&changed, Some(decoded.invitation_id()))?;
    let invitation =
        encode_full_invitation(decoded.invitation_id(), &route, decoded.expires_at_ms())?;
    let roundtrip = decode_invitation_entry(invitation.as_str(), now_ms)?
        .context("fixture invitation missing after encoding")?;
    let endpoint = decode_space_admission_continuation_endpoint(roundtrip.route())?;
    let mut contains = Vec::new();
    if let Some(checks) = input.get("contains_direct") {
        for address in checks.as_array().context("address checks required")? {
            let socket = address.as_str().context("socket check required")?.parse()?;
            contains.push(endpoint.ip_addrs().any(|address| *address == socket));
        }
    }
    let direct_count = endpoint
        .addrs
        .iter()
        .filter(|address| address.is_ip())
        .count();
    let relay_count = endpoint
        .addrs
        .iter()
        .filter(|address| address.is_relay())
        .count();
    serde_json::to_writer(
        io::stdout().lock(),
        &json!({
            "invitation": invitation.as_str(),
            "endpoint_identity_preserved": endpoint.id == original.id,
            "invitation_identity_preserved": roundtrip.invitation_id() == decoded.invitation_id(),
            "expiry_preserved": roundtrip.expires_at_ms() == decoded.expires_at_ms(),
            "contains_direct": contains,
            "direct_count": direct_count,
            "relay_count": relay_count,
        }),
    )?;
    io::stdout().lock().write_all(b"\n")?;
    Ok(())
}
