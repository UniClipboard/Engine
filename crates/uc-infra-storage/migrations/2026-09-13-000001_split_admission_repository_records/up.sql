CREATE TABLE admission_repository_record (
    lookup_token BLOB PRIMARY KEY NOT NULL CHECK (length(lookup_token) = 32),
    content_token BLOB NOT NULL CHECK (length(content_token) = 32),
    encrypted_payload BLOB NOT NULL
);
