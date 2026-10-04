-- Relationship records contain device names, identity fingerprints and transport
-- addresses. Keep the old tables only as a forward-migration source; the current
-- application reads and writes the sealed table exclusively.
ALTER TABLE space_member RENAME TO relationship_legacy_space_member;
ALTER TABLE trusted_peer RENAME TO relationship_legacy_trusted_peer;
ALTER TABLE peer_address RENAME TO relationship_legacy_peer_address;

CREATE TABLE encrypted_relationship (
    kind               TEXT NOT NULL CHECK (
        kind IN (
            'member',
            'trusted_peer',
            'peer_address',
            'candidate',
            'membership_announcement',
            'membership_outbox'
        )
    ),
    lookup_key         BLOB NOT NULL,
    payload_ciphertext BLOB NOT NULL,
    PRIMARY KEY (kind, lookup_key)
);

CREATE TABLE relationship_privacy_maintenance (
    id    INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    state TEXT NOT NULL CHECK (
        state IN ('pending_rows', 'pending_physical_purge', 'completed')
    )
);

INSERT INTO relationship_privacy_maintenance (id, state)
VALUES (1, 'pending_rows');
