use std::io;

pub(super) fn invalid_archive() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "profile backup archive is invalid",
    )
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileBackupArchiveError {
    #[error("profile backup storage failed")]
    Storage {
        #[source]
        source: io::Error,
    },
    #[error("profile backup source changed")]
    SourceChanged,
    #[error("profile backup does not match the requested archive")]
    StateChanged,
    #[error("profile backup directories overlap")]
    OverlappingDirectories,
}

impl From<io::Error> for ProfileBackupArchiveError {
    fn from(source: io::Error) -> Self {
        Self::Storage { source }
    }
}
