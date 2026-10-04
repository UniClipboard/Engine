-- Restore the previous kind constraint by rebuilding the table again.
CREATE TABLE encrypted_relationship_v1 (
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

INSERT INTO encrypted_relationship_v1 (kind, lookup_key, payload_ciphertext)
SELECT kind, lookup_key, payload_ciphertext FROM encrypted_relationship
WHERE EXISTS (
    SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'encrypted_relationship'
);

DROP TABLE IF EXISTS encrypted_relationship;

ALTER TABLE encrypted_relationship_v1 RENAME TO encrypted_relationship;
