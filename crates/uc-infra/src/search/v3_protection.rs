use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use hmac::{Hmac, Mac};
use sha2::Sha256;
use uc_core::crypto::aad;
use uc_core::crypto::domain::{Aad, Ciphertext, Plaintext};
use uc_core::ids::EntryId;
use uc_core::membership::ProtectionGroupId;
use uc_core::search::{SearchKey, SearchKeyContext, SearchProtectionRef};

use super::render_payload::{RenderFields, RENDER_PAYLOAD_V};
use crate::security::{ContentProtection, ProfileContentKeyVault};
use crate::space::InMemorySession;

const GROUP_REF_DOMAIN: &[u8] = b"uniclipboard/search/group-ref/v1\0";
const TERM_TAG_DOMAIN: &[u8] = b"uniclipboard/search/term-tag/v1\0";
const SEARCH_TAG_BYTES: usize = 32;
const MAX_INDEXED_GROUPS_PER_QUERY: usize = 128;
const MAX_QUERY_TERMS: usize = 64;
const MAX_NORMALIZED_TERM_BYTES: usize = 4096;

type HmacSha256 = Hmac<Sha256>;

/// 索引中持久化的保护组不透明引用。
///
/// 它由 profile 稳定搜索根和 `ProtectionGroupId` 计算，不能反推出保护组，
/// 只用于让查询请求 vault 中实际有文档的组。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SearchGroupRef([u8; SEARCH_TAG_BYTES]);

impl SearchGroupRef {
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, V3SearchProtectionError> {
        let value =
            bytes
                .try_into()
                .map_err(|source| V3SearchProtectionError::InvalidGroupReferences {
                    source: anyhow::Error::new(source).context("decode search group reference"),
                })?;
        Ok(Self(value))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for SearchGroupRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SearchGroupRef([REDACTED])")
    }
}

/// 一份文档在当前活动保护组下的完整索引密码结果。
pub struct IndexedSearchTerms {
    group_ref: SearchGroupRef,
    term_tags: Vec<Vec<u8>>,
}

impl std::fmt::Debug for IndexedSearchTerms {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IndexedSearchTerms")
            .field("group_ref", &"[REDACTED]")
            .field("term_count", &self.term_tags.len())
            .finish()
    }
}

impl IndexedSearchTerms {
    pub fn group_ref(&self) -> &SearchGroupRef {
        &self.group_ref
    }

    pub fn term_tags(&self) -> &[Vec<u8>] {
        &self.term_tags
    }
}

/// 每个查询词在索引实际涉及的保护组中的 tag alternatives。
///
/// 外层顺序与输入查询词一致。AND 查询必须要求每个外层集合至少命中一个，
/// 不能把所有组的 tag 扁平后按总 tag 数计数。
pub struct QueryTermTags {
    alternatives_by_term: Vec<Vec<Vec<u8>>>,
}

impl std::fmt::Debug for QueryTermTags {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QueryTermTags")
            .field("term_count", &self.alternatives_by_term.len())
            .finish()
    }
}

impl QueryTermTags {
    pub fn alternatives_by_term(&self) -> &[Vec<Vec<u8>>] {
        &self.alternatives_by_term
    }
}

#[derive(Debug, thiserror::Error)]
pub enum V3SearchProtectionError {
    #[error("no V3 search protection context is active")]
    NotActive {
        #[source]
        source: anyhow::Error,
    },
    #[error("the profile search catalog is unavailable")]
    CatalogUnavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("the indexed search group references are invalid")]
    InvalidGroupReferences {
        #[source]
        source: anyhow::Error,
    },
    #[error("V3 search tag construction failed")]
    Cryptography {
        #[source]
        source: anyhow::Error,
    },
    #[error("V3 search render payload encoding failed")]
    RenderEncode {
        #[source]
        source: anyhow::Error,
    },
    #[error("V3 search render payload is invalid")]
    RenderDecode {
        #[source]
        source: anyhow::Error,
    },
}

/// V3 本机搜索的唯一密码深模块。
///
/// 索引调用方只提交规范词项；本模块从活动 session 固定保护组。查询调用方
/// 只提交索引中 `DISTINCT` 取得的 opaque group ref；本模块用 vault 快照解析并
/// 为每个查询词生成分组 alternatives。render JSON 与业务 AAD 留在搜索模块，
/// key resolution、purpose 与 AEAD envelope 委托 `ContentProtection`。
pub struct V3SearchProtection {
    session: Arc<InMemorySession>,
    vault: Arc<ProfileContentKeyVault>,
    render_protection: ContentProtection,
}

impl V3SearchProtection {
    pub fn new(session: Arc<InMemorySession>, vault: Arc<ProfileContentKeyVault>) -> Self {
        let render_protection =
            ContentProtection::for_search(Arc::clone(&session), Arc::clone(&vault));
        Self {
            session,
            vault,
            render_protection,
        }
    }

    pub async fn index_terms(
        &self,
        normalized_terms: &[String],
    ) -> Result<IndexedSearchTerms, V3SearchProtectionError> {
        validate_terms(normalized_terms, false)?;
        let active = self
            .session
            .current_content_protection_key()
            .map_err(|source| V3SearchProtectionError::NotActive {
                source: anyhow::Error::new(source)
                    .context("resolve active search protection group"),
            })?;
        let catalog = self.vault.search_catalog().await.map_err(|source| {
            V3SearchProtectionError::CatalogUnavailable {
                source: anyhow::Error::new(source).context("load profile search catalog"),
            }
        })?;
        if !catalog
            .protection_groups()
            .contains(active.protection_group_id())
        {
            return Err(V3SearchProtectionError::CatalogUnavailable {
                source: anyhow::anyhow!(
                    "active protection group is absent from the search catalog"
                ),
            });
        }
        let group_ref = derive_group_ref(catalog.root_key(), active.protection_group_id())?;
        let term_tags = normalized_terms
            .iter()
            .map(|term| derive_term_tag(catalog.root_key(), active.protection_group_id(), term))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(IndexedSearchTerms {
            group_ref,
            term_tags,
        })
    }

    pub async fn active_key_context(&self) -> Result<SearchKeyContext, V3SearchProtectionError> {
        let active = self
            .session
            .current_content_protection_key()
            .map_err(|source| V3SearchProtectionError::NotActive {
                source: anyhow::Error::new(source)
                    .context("resolve active search protection group"),
            })?;
        let catalog = self.vault.search_catalog().await.map_err(|source| {
            V3SearchProtectionError::CatalogUnavailable {
                source: anyhow::Error::new(source).context("load profile search catalog"),
            }
        })?;
        if !catalog
            .protection_groups()
            .contains(active.protection_group_id())
        {
            return Err(V3SearchProtectionError::CatalogUnavailable {
                source: anyhow::anyhow!(
                    "active protection group is absent from the search catalog"
                ),
            });
        }
        let group_ref = derive_group_ref(catalog.root_key(), active.protection_group_id())?;
        let tagging_key = derive_tagging_key(catalog.root_key(), active.protection_group_id())?;
        let key = SearchKey::from_bytes(&tagging_key).map_err(|source| {
            V3SearchProtectionError::Cryptography {
                source: anyhow::Error::new(source).context("construct active search key"),
            }
        })?;
        let protection_ref =
            SearchProtectionRef::from_bytes(group_ref.as_bytes()).map_err(|source| {
                V3SearchProtectionError::Cryptography {
                    source: anyhow::Error::new(source)
                        .context("construct search protection reference"),
                }
            })?;
        Ok(SearchKeyContext::protected(key, protection_ref))
    }

    pub async fn query_terms(
        &self,
        indexed_group_refs: &[SearchGroupRef],
        normalized_terms: &[String],
    ) -> Result<QueryTermTags, V3SearchProtectionError> {
        validate_terms(normalized_terms, true)?;
        let unique_refs = indexed_group_refs.iter().cloned().collect::<BTreeSet<_>>();
        if unique_refs.len() > MAX_INDEXED_GROUPS_PER_QUERY {
            return Err(invalid_group_refs("too many indexed protection groups"));
        }
        let catalog = self.vault.search_catalog().await.map_err(|source| {
            V3SearchProtectionError::CatalogUnavailable {
                source: anyhow::Error::new(source).context("load profile search catalog"),
            }
        })?;
        let mut known = BTreeMap::new();
        for group in catalog.protection_groups() {
            known.insert(derive_group_ref(catalog.root_key(), group)?, group);
        }
        let groups = unique_refs
            .iter()
            .map(|group_ref| {
                known
                    .get(group_ref)
                    .copied()
                    .ok_or_else(|| invalid_group_refs("indexed protection group is unknown"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let alternatives_by_term = normalized_terms
            .iter()
            .map(|term| {
                groups
                    .iter()
                    .map(|group| derive_term_tag(catalog.root_key(), group, term))
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(QueryTermTags {
            alternatives_by_term,
        })
    }

    pub async fn seal_render(
        &self,
        entry_id: &EntryId,
        fields: &RenderFields,
    ) -> Result<Vec<u8>, V3SearchProtectionError> {
        if fields.v != RENDER_PAYLOAD_V {
            return Err(V3SearchProtectionError::RenderEncode {
                source: anyhow::anyhow!("search render schema version is unsupported"),
            });
        }
        let plaintext =
            serde_json::to_vec(fields).map_err(|source| V3SearchProtectionError::RenderEncode {
                source: anyhow::Error::new(source).context("encode V3 search render fields"),
            })?;
        self.render_protection
            .seal_for_active(
                &Plaintext::new(plaintext),
                &Aad::new(aad::for_search_render(entry_id)),
            )
            .await
            .map(Ciphertext::into_bytes)
            .map_err(|source| V3SearchProtectionError::RenderEncode {
                source: anyhow::Error::new(source).context("protect V3 search render fields"),
            })
    }

    pub async fn open_render(
        &self,
        entry_id: &EntryId,
        ciphertext: &[u8],
    ) -> Result<RenderFields, V3SearchProtectionError> {
        let plaintext = self
            .render_protection
            .open(
                &Ciphertext::new(ciphertext.to_vec()),
                &Aad::new(aad::for_search_render(entry_id)),
            )
            .await
            .map_err(|source| V3SearchProtectionError::RenderDecode {
                source: anyhow::Error::new(source).context("open V3 search render fields"),
            })?;
        let fields: RenderFields =
            serde_json::from_slice(plaintext.as_bytes()).map_err(|source| {
                V3SearchProtectionError::RenderDecode {
                    source: anyhow::Error::new(source).context("decode V3 search render fields"),
                }
            })?;
        if fields.v != RENDER_PAYLOAD_V {
            return Err(V3SearchProtectionError::RenderDecode {
                source: anyhow::anyhow!("search render schema version is unsupported"),
            });
        }
        Ok(fields)
    }
}

fn validate_terms(normalized_terms: &[String], query: bool) -> Result<(), V3SearchProtectionError> {
    if query && normalized_terms.len() > MAX_QUERY_TERMS {
        return Err(V3SearchProtectionError::Cryptography {
            source: anyhow::anyhow!("search query has too many normalized terms"),
        });
    }
    if normalized_terms
        .iter()
        .any(|term| term.is_empty() || term.len() > MAX_NORMALIZED_TERM_BYTES)
    {
        return Err(V3SearchProtectionError::Cryptography {
            source: anyhow::anyhow!("normalized search term length is invalid"),
        });
    }
    Ok(())
}

fn derive_group_ref(
    root_key: &crate::security::MasterKey,
    protection_group_id: &ProtectionGroupId,
) -> Result<SearchGroupRef, V3SearchProtectionError> {
    let mut mac = new_hmac(root_key)?;
    append_field(&mut mac, GROUP_REF_DOMAIN)?;
    append_field(&mut mac, protection_group_id.as_str().as_bytes())?;
    Ok(SearchGroupRef(mac.finalize().into_bytes().into()))
}

fn derive_term_tag(
    root_key: &crate::security::MasterKey,
    protection_group_id: &ProtectionGroupId,
    normalized_term: &str,
) -> Result<Vec<u8>, V3SearchProtectionError> {
    let tagging_key = derive_tagging_key(root_key, protection_group_id)?;
    let mut mac = HmacSha256::new_from_slice(&tagging_key).map_err(|source| {
        V3SearchProtectionError::Cryptography {
            source: anyhow::Error::new(source).context("initialize group search HMAC"),
        }
    })?;
    mac.update(normalized_term.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

fn derive_tagging_key(
    root_key: &crate::security::MasterKey,
    protection_group_id: &ProtectionGroupId,
) -> Result<[u8; 32], V3SearchProtectionError> {
    let mut mac = new_hmac(root_key)?;
    append_field(&mut mac, TERM_TAG_DOMAIN)?;
    append_field(&mut mac, protection_group_id.as_str().as_bytes())?;
    Ok(mac.finalize().into_bytes().into())
}

fn new_hmac(root_key: &crate::security::MasterKey) -> Result<HmacSha256, V3SearchProtectionError> {
    HmacSha256::new_from_slice(root_key.as_bytes()).map_err(|source| {
        V3SearchProtectionError::Cryptography {
            source: anyhow::Error::new(source).context("initialize profile search HMAC"),
        }
    })
}

fn append_field(mac: &mut HmacSha256, value: &[u8]) -> Result<(), V3SearchProtectionError> {
    let length =
        u64::try_from(value.len()).map_err(|source| V3SearchProtectionError::Cryptography {
            source: anyhow::Error::new(source).context("encode profile search HMAC field"),
        })?;
    mac.update(&length.to_be_bytes());
    mac.update(value);
    Ok(())
}

fn invalid_group_refs(context: &'static str) -> V3SearchProtectionError {
    V3SearchProtectionError::InvalidGroupReferences {
        source: anyhow::anyhow!(context),
    }
}
