//! Constants shared by the mDNS pairing publisher and resolver.
//!
//! Kept in one place so the on-wire contract (service name + TXT field
//! names) stays in lockstep across the two sides. A drift here would
//! manifest as "sponsor announces but joiner never matches" — silent and
//! hard to debug.
//!
//! ## Service-name isolation
//!
//! Pairing uses its **own** mDNS service name distinct from iroh's
//! `_irohv1._udp.local` peer-discovery flow. Reasons:
//!
//! * Pairing announces are window-scoped (only while a code is pending),
//!   whereas iroh peer discovery runs for the endpoint's lifetime. Mixing
//!   them would force the iroh discovery loop to filter pairing entries
//!   on every browse.
//! * Pairing TXT records carry different fields (`code_hash`, opaque
//!   ticket) that have no meaning to iroh's NodeId→IP loop.
//! * Privacy: pairing announces are deliberately scarce so a passive LAN
//!   observer cannot fingerprint a device by its pairing history.

/// mDNS service name pairing announces are published under.
///
/// `swarm_discovery::Discoverer::new` takes a bare service name; the crate
/// then appends `._udp.local` internally. So this constant is the bare
/// service part, **not** the fully-qualified DNS name.
pub const PAIR_SERVICE_NAME: &str = "uniclipboard-pair";

/// TXT key carrying `hex(blake3(code)[..8])` — a short hash prefix used by
/// the joiner to filter matching announces before fetching the heavier
/// ticket field. Hashing instead of broadcasting the raw code keeps a
/// passive observer from learning the code itself.
pub const TXT_CODE_HASH: &str = "ch";

/// TXT key carrying the sponsor's endpoint id (hex string). Adapters use
/// this to short-circuit "yes this is me" loops when the publisher and
/// resolver coexist in the same process during tests.
pub const TXT_NODE_ID: &str = "id";

/// TXT key carrying the number of ordered sponsor-ticket chunks.
pub const TXT_TICKET_CHUNK_COUNT: &str = "tn";

/// Maximum postcard+hex ticket size accepted from an mDNS announcement.
pub const MAX_TICKET_HEX_LEN: usize = 1_200;

const TXT_TICKET_CHUNK_PREFIX: &str = "t";
const TXT_TICKET_CHUNK_VALUE_LEN: usize = 240;
const MAX_TICKET_CHUNKS: usize = MAX_TICKET_HEX_LEN.div_ceil(TXT_TICKET_CHUNK_VALUE_LEN);

/// TXT key carrying the code's `expires_at_ms` as a decimal string.
/// Joiner uses this to filter out stale announces that linger past the
/// publisher's window (the publisher should stop announcing on expiry,
/// but mDNS cache propagation gives a small grace window).
pub const TXT_EXPIRES_AT_MS: &str = "ex";

/// Computes the short hash prefix broadcast in the `code_hash` TXT field.
///
/// 8 bytes ≈ 64 bits of entropy. Returned as lowercase hex so it round
/// trips through `swarm_discovery::Peer::txt_attribute` without case
/// normalisation surprises.
pub fn compute_code_hash(code: &str) -> String {
    let digest = blake3::hash(code.as_bytes());
    hex::encode(&digest.as_bytes()[..8])
}

/// Splits an ASCII hex endpoint ticket into bounded DNS-SD TXT attributes.
pub fn ticket_txt_attributes(ticket_hex: &str) -> Option<Vec<(String, Option<String>)>> {
    if ticket_hex.is_empty()
        || ticket_hex.len() > MAX_TICKET_HEX_LEN
        || !ticket_hex.len().is_multiple_of(2)
        || !ticket_hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }

    let chunks = ticket_hex.as_bytes().chunks(TXT_TICKET_CHUNK_VALUE_LEN);
    let count = chunks.len();
    if count == 0 || count > MAX_TICKET_CHUNKS {
        return None;
    }

    let mut attributes = Vec::with_capacity(count + 1);
    attributes.push((TXT_TICKET_CHUNK_COUNT.to_string(), Some(count.to_string())));
    for (index, chunk) in chunks.enumerate() {
        attributes.push((
            format!("{TXT_TICKET_CHUNK_PREFIX}{index}"),
            Some(std::str::from_utf8(chunk).ok()?.to_string()),
        ));
    }
    Some(attributes)
}

/// Reassembles a complete endpoint ticket from bounded DNS-SD TXT attributes.
pub fn ticket_from_txt_attributes<'a>(
    attributes: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> Option<String> {
    let mut expected_count = None;
    let mut chunks = [None; MAX_TICKET_CHUNKS];

    for (key, value) in attributes {
        if key == TXT_TICKET_CHUNK_COUNT {
            if expected_count.is_some() {
                return None;
            }
            let value = value?;
            let count = value.parse::<usize>().ok()?;
            if count == 0 || count > MAX_TICKET_CHUNKS || count.to_string() != value {
                return None;
            }
            expected_count = Some(count);
            continue;
        }

        let Some(index_text) = key.strip_prefix(TXT_TICKET_CHUNK_PREFIX) else {
            continue;
        };
        if index_text.is_empty() || !index_text.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let index = index_text.parse::<usize>().ok()?;
        if index >= MAX_TICKET_CHUNKS || index.to_string() != index_text || chunks[index].is_some()
        {
            return None;
        }
        let chunk = value?;
        if chunk.is_empty() || chunk.len() > TXT_TICKET_CHUNK_VALUE_LEN {
            return None;
        }
        chunks[index] = Some(chunk);
    }

    let expected_count = expected_count?;
    if chunks[expected_count..].iter().any(Option::is_some) {
        return None;
    }
    let mut ticket = String::new();
    for chunk in chunks[..expected_count].iter() {
        ticket.push_str(chunk.as_ref()?);
    }
    if ticket.len() > MAX_TICKET_HEX_LEN
        || !ticket.len().is_multiple_of(2)
        || !ticket.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some(ticket)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_hash_is_stable_and_16_hex_chars() {
        let h1 = compute_code_hash("ABCD-1234");
        let h2 = compute_code_hash("ABCD-1234");
        assert_eq!(h1, h2, "hash must be deterministic");
        assert_eq!(h1.len(), 16, "8 bytes -> 16 hex chars");
        assert!(
            h1.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "hash must be lowercase hex"
        );
    }

    #[test]
    fn code_hash_differs_for_different_codes() {
        assert_ne!(
            compute_code_hash("ABCD-1234"),
            compute_code_hash("ABCD-1235"),
        );
    }

    #[test]
    fn ticket_txt_attributes_split_and_restore_a_long_ticket() {
        let ticket = "ab".repeat(360);

        let attributes = ticket_txt_attributes(&ticket).expect("ticket attributes");
        assert!(attributes.len() > 2, "ticket must be split across fields");
        assert!(attributes
            .iter()
            .all(|(key, value)| { key.len() + value.as_deref().map_or(0, str::len) <= 254 }));

        assert_eq!(
            ticket_from_txt_attributes(
                attributes
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_deref()))
            ),
            Some(ticket)
        );
    }

    #[test]
    fn ticket_txt_attributes_reject_missing_or_out_of_range_chunks() {
        let ticket = "cd".repeat(300);
        let attributes = ticket_txt_attributes(&ticket).expect("ticket attributes");

        let missing = attributes
            .iter()
            .filter(|(key, _)| key != "t1")
            .map(|(key, value)| (key.as_str(), value.as_deref()));
        assert_eq!(ticket_from_txt_attributes(missing), None);

        let mut out_of_range = attributes.clone();
        out_of_range.push(("t9".to_string(), Some("ab".to_string())));
        assert_eq!(
            ticket_from_txt_attributes(
                out_of_range
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_deref()))
            ),
            None
        );
    }

    #[test]
    fn ticket_txt_attributes_enforce_the_total_ticket_limit() {
        let oversized = "ef".repeat((MAX_TICKET_HEX_LEN / 2) + 1);
        assert_eq!(ticket_txt_attributes(&oversized), None);
    }
}
