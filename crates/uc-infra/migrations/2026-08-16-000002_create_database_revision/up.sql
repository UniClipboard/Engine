CREATE TABLE uc_database_revision (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    revision INTEGER NOT NULL CHECK (revision >= 0)
);

INSERT INTO uc_database_revision (singleton_id, revision) VALUES (1, 0);
