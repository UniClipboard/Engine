use async_trait::async_trait;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::debug;
use uc_core::ports::clipboard::{SelfWriteAttribution, SelfWriteLedgerPort, SelfWriteMatch};
use uc_core::ClipboardChangeOrigin;

/// In-memory [`SelfWriteLedgerPort`] implementation.
///
/// Attribution is event-driven: a content-keyed record is consumed the moment a
/// change with the matching hash is observed, and a next-change fallback absorbs
/// the next change(s) of the same content kind. The per-record `expires_at` is a
/// pure garbage-collection backstop — it reclaims a record whose echo never
/// arrives (identical content, or a failed write), and never overrides the
/// next-event consumption above.
///
/// A single programmatic write can legitimately surface as MORE than one watcher
/// event: a platform may re-encode the bytes between write and echo (Windows
/// PNG→DIB→PNG), the OS may deliver two change notifications for one atomic
/// write, or an external clipboard manager may re-assert the content right
/// after the write. Under a strictly one-shot fallback, the first event consumed
/// every guard and the second was mis-attributed as a fresh `LocalCapture`,
/// which then dispatched back to the sender — the A↔B↔A image echo loop. To
/// attribute the whole echo set of one write instead of exactly one event:
///
/// - the next-change fallback carries an **echo budget** — two credits for
///   remote pushes (whose re-encoded echo the content hash provably cannot
///   match), one for local restores (fast, never re-encoded);
/// - a content match consumes one credit of the *paired* fallback (same guard
///   key) instead of deleting it, so a second, re-encoded echo is still caught;
/// - the fallback only absorbs changes of the **same content kind** (the
///   `image:` / `text:` / … prefix), so an unrelated copy of another kind can
///   no longer steal it;
/// - after the first credit is spent, the remaining credit lives only
///   [`ECHO_TAIL_TTL`] — long enough for the second OS event of the same write,
///   short enough that a later unrelated user copy is not swallowed.
///
/// Records store only the local-vs-remote [`SelfWriteAttribution`], not a
/// synthesized [`ClipboardChangeOrigin`]: the attribution is the real datum the
/// ledger tracks, it carries no peer identity that could split one snapshot into
/// two records under an equality check, and the mapping to a domain origin is
/// applied once, at read time.
pub(crate) struct InMemorySelfWriteLedger {
    state: Mutex<OriginStore>,
    echo_tail: Duration,
}

/// A content-keyed self-write: matched when a later change carries `snapshot_hash`.
/// Single-consume by design — an identical hash observed twice is either a
/// deliberate identical re-copy (text re-copied past the watcher's re-dedup
/// window, which must resurface as a local capture) or a duplicate event the
/// watcher should already have collapsed.
struct ContentRecord {
    snapshot_hash: String,
    attribution: SelfWriteAttribution,
    expires_at: Instant,
}

/// A next-change fallback for one programmatic write: absorbs re-encoded echoes
/// whose hash the content record can never match. `budget` caps how many OS
/// change events of the same content kind this fallback absorbs before it is
/// retired; see the module docs for why one is not enough for remote pushes.
struct NextChangeRecord {
    /// Content guard key of the write this fallback backs. Pairs the fallback to
    /// its write so a content match retires exactly the redundant one (not a
    /// concurrent write's), and keys arm-time de-duplication. Never consulted
    /// when the fallback is consumed by an unmatched change — consumption is
    /// kind-scoped instead, to catch re-encoded echoes under an unknown hash
    /// while ignoring unrelated content of a different kind.
    guard_key: String,
    /// Content kind of `guard_key` (`image:` / `text:` / `files:` /
    /// `rich-text:`, or `None` for bare snapshot hashes). A fallback only
    /// matches an observed change of the same kind.
    kind: Option<String>,
    attribution: SelfWriteAttribution,
    expires_at: Instant,
    budget: u8,
}

struct OriginStore {
    /// FIFO of pending next-change fallbacks. A queue (not a single slot) so two
    /// concurrent writes do not clobber each other's fallback — same-attribution
    /// fallbacks are interchangeable, so consuming the front of a kind-matching
    /// prefix is always correct.
    next_changes: VecDeque<NextChangeRecord>,
    content_records: VecDeque<ContentRecord>,
}

/// Cap on retained content records; oldest evicted past this.
const CONTENT_RECORD_MAX: usize = 256;
/// Cap on pending next-change fallbacks; oldest evicted past this. Far above the
/// realistic number of in-flight programmatic writes — a runaway backstop only.
const NEXT_CHANGE_MAX: usize = 64;

/// Fallback echo budget for a REMOTE programmatic write. The first credit is
/// the re-encoded echo itself; the second covers the observed double-event
/// shapes (a second OS change notification, a clipboard manager re-asserting
/// right after the write). One-shot attribution turned that second event into a
/// LocalCapture that bounced back to the sender, so remote pushes get two.
const REMOTE_ECHO_BUDGET: u8 = 2;

/// Fallback echo budget for a LOCAL programmatic write. Local echoes are fast
/// and never OS-re-encoded; the historical one-shot semantics are preserved so
/// a leftover fallback cannot swallow the user's next copy.
const LOCAL_ECHO_BUDGET: u8 = 1;

/// How long a fallback's remaining credit survives after its first echo is
/// consumed. The second OS event of the same write arrives essentially
/// together with the first; a later, unrelated same-kind copy must fall
/// through to `LocalCapture`.
const ECHO_TAIL_TTL: Duration = Duration::from_secs(5);

fn fallback_budget(attribution: SelfWriteAttribution) -> u8 {
    match attribution {
        SelfWriteAttribution::Local => LOCAL_ECHO_BUDGET,
        SelfWriteAttribution::Remote => REMOTE_ECHO_BUDGET,
    }
}

/// Content-kind prefix of a key produced by `origin_guard_key()` /
/// `meaningful_origin_key()`: `image:` / `text:` / `files:` / `rich-text:`,
/// or `None` for bare snapshot hashes (which carry no prefix).
fn kind_of(key: &str) -> Option<&str> {
    key.split_once(':').map(|(prefix, _)| prefix)
}

fn attribution_to_origin(attribution: SelfWriteAttribution) -> ClipboardChangeOrigin {
    match attribution {
        SelfWriteAttribution::Local => ClipboardChangeOrigin::LocalRestore,
        // Remote resolves to the anonymous variant: the ledger tracks no peer
        // identity, so `from_device` is always `None`.
        SelfWriteAttribution::Remote => ClipboardChangeOrigin::remote_push_anonymous(),
    }
}

impl InMemorySelfWriteLedger {
    pub(crate) fn new() -> Self {
        Self::with_echo_tail(ECHO_TAIL_TTL)
    }

    /// Construct with a custom tail TTL for the fallback's last echo credit.
    /// Intended for tests that exercise tail expiry without literally waiting
    /// [`ECHO_TAIL_TTL`]; production uses [`Self::new`].
    pub(crate) fn with_echo_tail(echo_tail: Duration) -> Self {
        Self {
            state: Mutex::new(OriginStore {
                next_changes: VecDeque::new(),
                content_records: VecDeque::new(),
            }),
            echo_tail,
        }
    }

    /// Drop every record whose backstop has elapsed. `retain` (not
    /// pop-front-while-expired) is required because records carry different TTLs
    /// (local vs remote budgets), so insertion order is not expiry order.
    fn prune_expired(store: &mut OriginStore, now: Instant) {
        store.next_changes.retain(|r| now <= r.expires_at);
        store.content_records.retain(|r| now <= r.expires_at);
    }

    fn remember_content_record(
        store: &mut OriginStore,
        snapshot_hash: String,
        attribution: SelfWriteAttribution,
        expires_at: Instant,
    ) {
        if let Some(existing) = store
            .content_records
            .iter_mut()
            .find(|r| r.snapshot_hash == snapshot_hash && r.attribution == attribution)
        {
            existing.expires_at = expires_at;
            return;
        }

        store.content_records.push_back(ContentRecord {
            snapshot_hash,
            attribution,
            expires_at,
        });
        while store.content_records.len() > CONTENT_RECORD_MAX {
            store.content_records.pop_front();
        }
    }

    /// Spend one credit of the fallback at `idx`; removes the record once its
    /// budget is exhausted, otherwise shortens its lifetime to the echo tail so
    /// the remaining credit only absorbs the same write's follow-up event.
    fn consume_fallback_credit(store: &mut OriginStore, idx: usize, now: Instant, tail: Duration) {
        let record = &mut store.next_changes[idx];
        record.budget = record.budget.saturating_sub(1);
        if record.budget == 0 {
            store.next_changes.remove(idx);
        } else {
            record.expires_at = record.expires_at.min(now + tail);
        }
    }
}

#[async_trait]
impl SelfWriteLedgerPort for InMemorySelfWriteLedger {
    async fn record_self_write(
        &self,
        matching: SelfWriteMatch,
        attribution: SelfWriteAttribution,
        ttl: Duration,
    ) {
        let now = Instant::now();
        let expires_at = now.checked_add(ttl).unwrap_or(now);
        let mut state = self.state.lock().await;
        Self::prune_expired(&mut state, now);
        match matching {
            SelfWriteMatch::ByContent(snapshot_hash) => {
                debug!(
                    snapshot_hash = %snapshot_hash,
                    ?attribution,
                    ttl_ms = ttl.as_millis(),
                    "self_write_ledger record content guard"
                );
                Self::remember_content_record(&mut state, snapshot_hash, attribution, expires_at);
            }
            SelfWriteMatch::ByNextChange(guard_key) => {
                debug!(
                    ?attribution,
                    ttl_ms = ttl.as_millis(),
                    "self_write_ledger record next-change fallback"
                );
                // De-duplicate by (guard_key, attribution). One write may arm
                // its fallback more than once — the same snapshot written twice
                // coalesces into a single OS echo, so a duplicated fallback
                // would linger past that lone echo and swallow the next genuine
                // change. Same key+attribution is the same write, so refreshing
                // the existing record's backstop and resetting its echo budget
                // is correct; a different key (a concurrent write) keeps its own
                // independent fallback.
                let kind = kind_of(&guard_key).map(str::to_owned);
                if let Some(existing) = state
                    .next_changes
                    .iter_mut()
                    .find(|r| r.guard_key == guard_key && r.attribution == attribution)
                {
                    existing.expires_at = expires_at;
                    existing.budget = fallback_budget(attribution);
                } else {
                    state.next_changes.push_back(NextChangeRecord {
                        guard_key,
                        kind,
                        attribution,
                        expires_at,
                        budget: fallback_budget(attribution),
                    });
                    while state.next_changes.len() > NEXT_CHANGE_MAX {
                        state.next_changes.pop_front();
                    }
                }
            }
        }
    }

    async fn attribute_observed_change(&self, snapshot_hash: &str) -> ClipboardChangeOrigin {
        let mut state = self.state.lock().await;
        let now = Instant::now();
        Self::prune_expired(&mut state, now);

        if let Some(idx) = state
            .content_records
            .iter()
            .position(|r| r.snapshot_hash == snapshot_hash)
        {
            if let Some(stored) = state.content_records.remove(idx) {
                // The content match resolved one echo of this write. Spend one
                // credit of the fallback paired to the SAME write (same guard
                // key, same attribution) instead of deleting it: the OS may
                // deliver a second, re-encoded change event for this very write,
                // and that event must still be attributed rather than becoming
                // a LocalCapture that bounces back to the sender. Pairing by
                // guard_key (not merely attribution) keeps a concurrent write's
                // independent fallback untouched.
                if let Some(fidx) = state.next_changes.iter().position(|r| {
                    r.guard_key == snapshot_hash && r.attribution == stored.attribution
                }) {
                    Self::consume_fallback_credit(&mut state, fidx, now, self.echo_tail);
                }
                debug!(
                    snapshot_hash = %snapshot_hash,
                    ?stored.attribution,
                    "self_write_ledger content guard matched"
                );
                return attribution_to_origin(stored.attribution);
            }
        }

        // Pruning above already dropped expired fallbacks, so any remaining
        // record is live. Absorb the change with the first kind-matching
        // fallback (FIFO within the kind): a re-encoded image echo must not be
        // consumed by a pending text fallback, nor vice versa.
        let observed_kind = kind_of(snapshot_hash);
        if let Some(idx) = state
            .next_changes
            .iter()
            .position(|r| r.kind.as_deref() == observed_kind)
        {
            let attribution = state.next_changes[idx].attribution;
            Self::consume_fallback_credit(&mut state, idx, now, self.echo_tail);
            debug!(
                snapshot_hash = %snapshot_hash,
                ?attribution,
                "self_write_ledger next-change fallback matched"
            );
            return attribution_to_origin(attribution);
        }

        debug!(
            snapshot_hash = %snapshot_hash,
            "self_write_ledger no guard matched; treating as local capture"
        );

        ClipboardChangeOrigin::LocalCapture
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: Duration = Duration::from_secs(60);

    #[tokio::test]
    async fn content_match_resolves_and_consumes() {
        let ledger = InMemorySelfWriteLedger::new();
        ledger
            .record_self_write(
                SelfWriteMatch::ByContent("h1".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;

        assert_eq!(
            ledger.attribute_observed_change("h1").await,
            ClipboardChangeOrigin::remote_push_anonymous()
        );
        // Content records stay single-consume: a second observation of the same
        // hash is a fresh capture (e.g. a deliberate identical re-copy).
        assert_eq!(
            ledger.attribute_observed_change("h1").await,
            ClipboardChangeOrigin::LocalCapture
        );
    }

    #[tokio::test]
    async fn local_attribution_maps_to_local_restore() {
        let ledger = InMemorySelfWriteLedger::new();
        ledger
            .record_self_write(
                SelfWriteMatch::ByContent("h1".into()),
                SelfWriteAttribution::Local,
                LONG,
            )
            .await;
        assert_eq!(
            ledger.attribute_observed_change("h1").await,
            ClipboardChangeOrigin::LocalRestore
        );
    }

    #[tokio::test]
    async fn no_record_resolves_to_local_capture() {
        let ledger = InMemorySelfWriteLedger::new();
        assert_eq!(
            ledger.attribute_observed_change("whatever").await,
            ClipboardChangeOrigin::LocalCapture
        );
    }

    #[tokio::test]
    async fn next_change_fallback_matches_reencoded_hash() {
        let ledger = InMemorySelfWriteLedger::new();
        ledger
            .record_self_write(
                SelfWriteMatch::ByNextChange("h1".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;
        // A re-encoded echo arrives under a hash the content record never saw.
        assert_eq!(
            ledger.attribute_observed_change("re-encoded-hash").await,
            ClipboardChangeOrigin::remote_push_anonymous()
        );
    }

    #[tokio::test]
    async fn remote_fallback_absorbs_two_echoes_then_retires() {
        // The echo-set shape the loop fix targets: a remote write whose OS
        // delivers TWO change events — the second re-encoded under an unknown
        // hash. Both must attribute to RemotePush; only a third, unrelated
        // change resolves to a fresh local capture.
        let ledger = InMemorySelfWriteLedger::new();
        ledger
            .record_self_write(
                SelfWriteMatch::ByNextChange("h1".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;
        assert_eq!(
            ledger.attribute_observed_change("h1-reencoded-1").await,
            ClipboardChangeOrigin::remote_push_anonymous()
        );
        assert_eq!(
            ledger.attribute_observed_change("h1-reencoded-2").await,
            ClipboardChangeOrigin::remote_push_anonymous()
        );
        assert_eq!(
            ledger.attribute_observed_change("unrelated").await,
            ClipboardChangeOrigin::LocalCapture
        );
    }

    #[tokio::test]
    async fn content_match_keeps_paired_fallback_for_second_echo() {
        // The echo-set shape with a content hit first: echo #1 carries the
        // exact write hash (content match), echo #2 is the re-encoded variant.
        // The content match spends one credit of the paired fallback, so echo
        // #2 still attributes as RemotePush instead of bouncing as a
        // LocalCapture.
        let ledger = InMemorySelfWriteLedger::new();
        ledger
            .record_self_write(
                SelfWriteMatch::ByContent("h1".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;
        ledger
            .record_self_write(
                SelfWriteMatch::ByNextChange("h1".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;

        assert_eq!(
            ledger.attribute_observed_change("h1").await,
            ClipboardChangeOrigin::remote_push_anonymous()
        );
        assert_eq!(
            ledger.attribute_observed_change("h1-reencoded").await,
            ClipboardChangeOrigin::remote_push_anonymous()
        );
        // Budget exhausted: a genuine, unrelated copy resolves to a fresh
        // capture.
        assert_eq!(
            ledger.attribute_observed_change("unrelated").await,
            ClipboardChangeOrigin::LocalCapture
        );
    }

    #[tokio::test]
    async fn content_match_clears_one_paired_fallback_for_local_write() {
        // Local restores keep one-shot semantics: the content match exhausts
        // the paired fallback's budget of one, so it cannot linger and swallow
        // the user's next genuine copy.
        let ledger = InMemorySelfWriteLedger::new();
        ledger
            .record_self_write(
                SelfWriteMatch::ByContent("h1".into()),
                SelfWriteAttribution::Local,
                LONG,
            )
            .await;
        ledger
            .record_self_write(
                SelfWriteMatch::ByNextChange("h1".into()),
                SelfWriteAttribution::Local,
                LONG,
            )
            .await;

        assert_eq!(
            ledger.attribute_observed_change("h1").await,
            ClipboardChangeOrigin::LocalRestore
        );
        assert_eq!(
            ledger.attribute_observed_change("a-real-user-copy").await,
            ClipboardChangeOrigin::LocalCapture
        );
    }

    #[tokio::test]
    async fn fallback_tail_expires_after_first_echo() {
        // After the first echo is consumed, the remaining credit lives only
        // ECHO_TAIL_TTL — a later same-kind change is a genuine user copy.
        let ledger = InMemorySelfWriteLedger::with_echo_tail(Duration::ZERO);
        ledger
            .record_self_write(
                SelfWriteMatch::ByNextChange("h1".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;
        assert_eq!(
            ledger.attribute_observed_change("echo-1").await,
            ClipboardChangeOrigin::remote_push_anonymous()
        );
        // Tail expired immediately (zero TTL): the leftover credit is gone.
        assert_eq!(
            ledger.attribute_observed_change("a-real-user-copy").await,
            ClipboardChangeOrigin::LocalCapture
        );
    }

    #[tokio::test]
    async fn fallback_only_absorbs_same_content_kind() {
        // An image write's fallback must not be consumed by an unrelated text
        // change that arrives before the echo, and vice versa.
        let ledger = InMemorySelfWriteLedger::new();
        ledger
            .record_self_write(
                SelfWriteMatch::ByNextChange("image:guard-h".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;

        assert_eq!(
            ledger
                .attribute_observed_change("text:unrelated-copy")
                .await,
            ClipboardChangeOrigin::LocalCapture
        );
        assert_eq!(
            ledger
                .attribute_observed_change("image:reencoded-echo")
                .await,
            ClipboardChangeOrigin::remote_push_anonymous()
        );
    }

    /// Regression: two concurrent remote writes must not clobber each other's
    /// fallback. Under the old single-slot design, write2 overwrote write1's
    /// override and the content match then cleared it, so write2's re-encoded
    /// echo fell through to `LocalCapture` and bounced back to the sender.
    #[tokio::test]
    async fn concurrent_remote_writes_keep_independent_fallbacks() {
        let ledger = InMemorySelfWriteLedger::new();
        // write1
        ledger
            .record_self_write(
                SelfWriteMatch::ByContent("h1".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;
        ledger
            .record_self_write(
                SelfWriteMatch::ByNextChange("h1".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;
        // write2 (interleaved before either echo lands)
        ledger
            .record_self_write(
                SelfWriteMatch::ByContent("h2".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;
        ledger
            .record_self_write(
                SelfWriteMatch::ByNextChange("h2".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;

        // write1 echoes back unchanged (content match) → spends one credit of
        // write1's paired fallback; write2's fallback is untouched.
        assert_eq!(
            ledger.attribute_observed_change("h1").await,
            ClipboardChangeOrigin::remote_push_anonymous()
        );
        // write2 echoes back RE-ENCODED (hash differs) → must still resolve to
        // remote, not LocalCapture.
        assert_eq!(
            ledger.attribute_observed_change("h2-reencoded").await,
            ClipboardChangeOrigin::remote_push_anonymous()
        );
    }

    /// Regression: an inbound sync that writes the SAME snapshot twice (e.g. an
    /// apply followed by an active-state rebroadcast) arms two content records
    /// and two fallbacks under one attribution. The OS coalesces the two
    /// identical writes into a single observed echo, so only one content match
    /// fires. The fallback's echo budget is bounded: after the coalesced echo
    /// the budget is spent, and a genuine, unrelated user copy resolves to a
    /// fresh capture.
    #[tokio::test]
    async fn double_write_same_content_does_not_leak_fallback() {
        let ledger = InMemorySelfWriteLedger::new();
        // First write of the snapshot: content guard + paired fallback.
        ledger
            .record_self_write(
                SelfWriteMatch::ByContent("h".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;
        ledger
            .record_self_write(
                SelfWriteMatch::ByNextChange("h".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;
        // Second write of the IDENTICAL snapshot (same hash, same attribution).
        ledger
            .record_self_write(
                SelfWriteMatch::ByContent("h".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;
        ledger
            .record_self_write(
                SelfWriteMatch::ByNextChange("h".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;

        // The OS merged both writes into one observed echo.
        assert_eq!(
            ledger.attribute_observed_change("h").await,
            ClipboardChangeOrigin::remote_push_anonymous()
        );
        // The remaining echo credit may absorb one more event of the same
        // write's echo set…
        assert_eq!(
            ledger.attribute_observed_change("h-reencoded").await,
            ClipboardChangeOrigin::remote_push_anonymous()
        );
        // …but a genuine, unrelated user copy must resolve to a fresh capture,
        // NOT be eaten by a leaked fallback from the duplicated write.
        assert_eq!(
            ledger.attribute_observed_change("a-real-user-copy").await,
            ClipboardChangeOrigin::LocalCapture
        );
    }

    #[tokio::test]
    async fn expired_records_are_pruned_regardless_of_ttl_order() {
        let ledger = InMemorySelfWriteLedger::new();
        // Short-TTL record inserted FIRST, long-TTL record SECOND: insertion
        // order is not expiry order, so pop-front-while-expired would be wrong.
        ledger
            .record_self_write(
                SelfWriteMatch::ByContent("short".into()),
                SelfWriteAttribution::Local,
                Duration::from_millis(1),
            )
            .await;
        ledger
            .record_self_write(
                SelfWriteMatch::ByContent("long".into()),
                SelfWriteAttribution::Remote,
                LONG,
            )
            .await;

        tokio::time::sleep(Duration::from_millis(20)).await;

        // The short record expired; the long one behind it must survive.
        assert_eq!(
            ledger.attribute_observed_change("short").await,
            ClipboardChangeOrigin::LocalCapture
        );
        assert_eq!(
            ledger.attribute_observed_change("long").await,
            ClipboardChangeOrigin::remote_push_anonymous()
        );
    }
}
