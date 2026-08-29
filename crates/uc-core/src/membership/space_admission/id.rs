macro_rules! define_redacted_id {
    ($name:ident, $size:expr) => {
        #[derive(
            Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
        )]
        pub struct $name([u8; $size]);

        impl $name {
            pub fn from_bytes(bytes: [u8; $size]) -> Option<Self> {
                if bytes == [0; $size] {
                    None
                } else {
                    Some(Self(bytes))
                }
            }

            pub const fn as_bytes(&self) -> &[u8; $size] {
                &self.0
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    };
}

define_redacted_id!(SpaceAdmissionId, 32);
define_redacted_id!(JoinId, 16);
define_redacted_id!(AdmissionMessageId, 32);
define_redacted_id!(InvitationId, 32);
define_redacted_id!(AdmissionChannelPeerId, 32);
