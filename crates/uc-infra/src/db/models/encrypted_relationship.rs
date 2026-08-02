use crate::db::schema::encrypted_relationship;
use diesel::prelude::*;

#[derive(Debug, Queryable, Selectable)]
#[diesel(table_name = encrypted_relationship)]
pub struct EncryptedRelationshipRow {
    pub kind: String,
    pub lookup_key: Vec<u8>,
    pub payload_ciphertext: Vec<u8>,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = encrypted_relationship)]
pub struct NewEncryptedRelationshipRow {
    pub kind: String,
    pub lookup_key: Vec<u8>,
    pub payload_ciphertext: Vec<u8>,
}
