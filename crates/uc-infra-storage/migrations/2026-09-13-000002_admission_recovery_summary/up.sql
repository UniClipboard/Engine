CREATE TABLE admission_recovery_summary (
    lookup_token BLOB PRIMARY KEY NOT NULL,
    content_token BLOB NOT NULL,
    encrypted_payload BLOB NOT NULL
);

CREATE TRIGGER invalidate_admission_recovery_summary_after_update
AFTER UPDATE OF content_token, encrypted_payload ON admission_repository_record
BEGIN
    DELETE FROM admission_recovery_summary WHERE lookup_token = OLD.lookup_token;
END;

CREATE TRIGGER invalidate_admission_recovery_summary_after_delete
AFTER DELETE ON admission_repository_record
BEGIN
    DELETE FROM admission_recovery_summary WHERE lookup_token = OLD.lookup_token;
END;
