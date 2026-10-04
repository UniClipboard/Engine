-- Encrypted relationship rows gained a kind for security updates the local
-- device already applied and may relay to peers behind on the group epoch.
-- SQLite cannot alter a CHECK constraint in place, so rebuild the table
-- while preserving the sealed payloads untouched. Databases that never
-- created the sealed relationship table (older file-transfer-only stores)
-- skip the data move but still end up with the new schema.
CREATE TABLE encrypted_relationship_v2 (
    kind               TEXT NOT NULL CHECK (
        kind IN (
            'member',
            'trusted_peer',
            'peer_address',
            'candidate',
            'membership_announcement',
            'membership_outbox',
            'membership_applied_security_update'
        )
    ),
    lookup_key         BLOB NOT NULL,
    payload_ciphertext BLOB NOT NULL,
    PRIMARY KEY (kind, lookup_key)
);

INSERT INTO encrypted_relationship_v2 (kind, lookup_key, payload_ciphertext)
SELECT kind, lookup_key, payload_ciphertext FROM encrypted_relationship
WHERE EXISTS (
    SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'encrypted_relationship'
);

DROP TABLE IF EXISTS encrypted_relationship;

ALTER TABLE encrypted_relationship_v2 RENAME TO encrypted_relationship;
