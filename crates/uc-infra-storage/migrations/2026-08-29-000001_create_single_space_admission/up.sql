CREATE TABLE membership_ledger_state (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    encrypted_payload BLOB NOT NULL
);

CREATE TABLE space_admission_credentials (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    encrypted_payload BLOB NOT NULL
);
