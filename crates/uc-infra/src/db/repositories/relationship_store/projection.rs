use super::*;
use std::collections::BTreeMap;
use uc_application::deps::{ApplyMembershipProjectionError, MembershipProjectionPlan};

impl<E: DbExecutor> EncryptedRelationshipStore<E> {
    pub(crate) async fn reconcile_membership_projection<F>(
        &self,
        plan: &MembershipProjectionPlan,
        verify_ledger: F,
    ) -> Result<(), ApplyMembershipProjectionError>
    where
        F: FnOnce(&mut diesel::SqliteConnection) -> Result<(), ApplyMembershipProjectionError>
            + Send,
    {
        let cipher = self.ready_cipher().await.map_err(dependency)?;
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    verify_ledger(conn)?;
                    let rows = encrypted_relationship::table
                        .filter(encrypted_relationship::kind.eq_any([
                            "member",
                            "trusted_peer",
                            "peer_address",
                        ]))
                        .select(EncryptedRelationshipRow::as_select())
                        .load::<EncryptedRelationshipRow>(conn)?;
                    let mut existing = rows
                        .into_iter()
                        .map(|row| ((row.kind, row.lookup_key), row.payload_ciphertext))
                        .collect::<BTreeMap<_, _>>();
                    let mut desired = Vec::new();
                    for facts in &plan.members {
                        let member_key =
                            cipher.lookup_key(RelationshipKind::Member, facts.device_id.as_str());
                        let member = existing
                            .get(&("member".to_owned(), member_key))
                            .map(|bytes| {
                                cipher
                                    .open(
                                        RelationshipKind::Member,
                                        &cipher.lookup_key(
                                            RelationshipKind::Member,
                                            facts.device_id.as_str(),
                                        ),
                                        bytes,
                                    )
                                    .and_then(|bytes| decode_member(&bytes))
                            })
                            .transpose()?;
                        let member = match member {
                            Some(member)
                                if member.identity_fingerprint == facts.identity_fingerprint =>
                            {
                                member
                            }
                            _ => SpaceMember {
                                device_id: facts.device_id.clone(),
                                device_name: facts.device_name.clone(),
                                identity_fingerprint: facts.identity_fingerprint.clone(),
                                joined_at: chrono::DateTime::<Utc>::UNIX_EPOCH,
                                sync_preferences: MemberSyncPreferences::default(),
                            },
                        };
                        desired.push((
                            RelationshipKind::Member,
                            facts.device_id.as_str(),
                            encode_member(&member)?,
                        ));
                        if plan.trusted_device_ids.contains(&facts.device_id) {
                            let key = cipher.lookup_key(
                                RelationshipKind::TrustedPeer,
                                facts.device_id.as_str(),
                            );
                            let previous = existing
                                .get(&("trusted_peer".to_owned(), key.clone()))
                                .map(|bytes| {
                                    cipher
                                        .open(RelationshipKind::TrustedPeer, &key, bytes)
                                        .and_then(|bytes| decode_trusted_peer(&bytes))
                                })
                                .transpose()?;
                            let peer = TrustedPeer {
                                local_device_id: plan.local_device_id.clone(),
                                peer_device_id: facts.device_id.clone(),
                                peer_fingerprint: facts.identity_fingerprint.clone(),
                                trusted_at: previous
                                    .map(|peer| peer.trusted_at)
                                    .unwrap_or(chrono::DateTime::<Utc>::UNIX_EPOCH),
                            };
                            desired.push((
                                RelationshipKind::TrustedPeer,
                                facts.device_id.as_str(),
                                encode_trusted_peer(&peer)?,
                            ));
                        }
                        if facts.device_id != plan.local_device_id {
                            let key = cipher.lookup_key(
                                RelationshipKind::PeerAddress,
                                facts.device_id.as_str(),
                            );
                            let previous = existing
                                .get(&("peer_address".to_owned(), key.clone()))
                                .map(|bytes| {
                                    cipher
                                        .open(RelationshipKind::PeerAddress, &key, bytes)
                                        .and_then(|bytes| decode_peer_address(&bytes))
                                })
                                .transpose()?;
                            if let Some(address) = previous.or_else(|| {
                                (!facts.transport_address_blob.is_empty()).then(|| {
                                    PeerAddressRecord {
                                        device_id: facts.device_id.clone(),
                                        addr_blob: facts.transport_address_blob.clone(),
                                        observed_at: chrono::DateTime::<Utc>::UNIX_EPOCH,
                                    }
                                })
                            }) {
                                desired.push((
                                    RelationshipKind::PeerAddress,
                                    facts.device_id.as_str(),
                                    encode_peer_address(&address)?,
                                ));
                            }
                        }
                    }
                    for (kind, identity, plaintext) in desired {
                        let lookup_key = cipher.lookup_key(kind, identity);
                        let previous =
                            existing.remove(&(kind.as_str().to_owned(), lookup_key.clone()));
                        if previous
                            .as_ref()
                            .map(|bytes| cipher.open(kind, &lookup_key, bytes))
                            .transpose()?
                            .as_ref()
                            == Some(&plaintext)
                        {
                            continue;
                        }
                        let payload_ciphertext = cipher.seal(kind, &lookup_key, &plaintext)?;
                        diesel::insert_into(encrypted_relationship::table)
                            .values(NewEncryptedRelationshipRow {
                                kind: kind.as_str().to_owned(),
                                lookup_key,
                                payload_ciphertext: payload_ciphertext.clone(),
                            })
                            .on_conflict((
                                encrypted_relationship::kind,
                                encrypted_relationship::lookup_key,
                            ))
                            .do_update()
                            .set(encrypted_relationship::payload_ciphertext.eq(payload_ciphertext))
                            .execute(conn)?;
                    }
                    for ((kind, key), _) in existing {
                        diesel::delete(
                            encrypted_relationship::table
                                .filter(encrypted_relationship::kind.eq(kind))
                                .filter(encrypted_relationship::lookup_key.eq(key)),
                        )
                        .execute(conn)?;
                    }
                    Ok(())
                })
            })
            .map_err(
                |error| match error.downcast::<ApplyMembershipProjectionError>() {
                    Ok(error) => error,
                    Err(source) => ApplyMembershipProjectionError::Dependency { source },
                },
            )
    }
}

fn dependency(error: RelationshipStoreError) -> ApplyMembershipProjectionError {
    ApplyMembershipProjectionError::Dependency {
        source: anyhow::Error::new(error),
    }
}
