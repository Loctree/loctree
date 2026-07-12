//! Multi-token auth with per-token scopes and namespace ACLs.
//!
//! Ported from `rust-memex` and adapted for the loctree MCP SaaS surface.
//! Each token is hashed with argon2id at rest. Plaintext is shown once on
//! creation and never persisted.

#![allow(dead_code)]

mod scope;

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use chrono::{DateTime, Utc};
use password_hash::rand_core::OsRng;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

pub(crate) use scope::Scope;

const DEFAULT_TOKEN_STORE_PATH: &str = "~/.rmcp-servers/loctree-mcp/tokens.json";

fn argon2id() -> Argon2<'static> {
    Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::default())
}

/// A single token entry persisted in tokens.json (v2 schema).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TokenEntry {
    /// Human-readable identifier (for example, "monika-iphone").
    pub id: String,
    /// Argon2id hash of the token. Plaintext is never stored.
    pub token_hash: String,
    /// Permission scopes granted to this token.
    pub scopes: Vec<Scope>,
    /// Namespace ACL. `["*"]` means all namespaces.
    pub namespaces: Vec<String>,
    /// Optional expiry timestamp. `None` means never expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// Human-readable description.
    pub description: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

impl TokenEntry {
    /// Check if the token has expired.
    pub(crate) fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| Utc::now() > expires_at)
    }

    /// Check if the token grants access to a namespace.
    pub(crate) fn has_namespace_access(&self, namespace: &str) -> bool {
        self.namespaces
            .iter()
            .any(|allowed| allowed == "*" || allowed == namespace)
    }

    /// Check if the token has a required scope.
    pub(crate) fn has_scope(&self, scope: &Scope) -> bool {
        self.scopes.iter().any(|granted| granted.grants(scope))
    }
}

/// Version 2 token store schema, persisted as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TokenStoreV2 {
    pub version: u32,
    pub tokens: Vec<TokenEntry>,
}

impl Default for TokenStoreV2 {
    fn default() -> Self {
        Self {
            version: 2,
            tokens: Vec::new(),
        }
    }
}

/// Version 1 schema (legacy) for migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenEntryV1 {
    namespace: String,
    token: String,
    created_at: u64,
    description: Option<String>,
}

/// Persistent token store backed by `tokens.json`.
#[derive(Debug)]
pub(crate) struct TokenStoreFile {
    store: Arc<RwLock<TokenStoreV2>>,
    store_path: String,
}

impl TokenStoreFile {
    /// Create a new token store at the given path.
    pub(crate) fn new(store_path: String) -> Self {
        Self {
            store: Arc::new(RwLock::new(TokenStoreV2::default())),
            store_path,
        }
    }

    /// Default loctree-mcp token store path.
    pub(crate) fn default_store_path() -> String {
        DEFAULT_TOKEN_STORE_PATH.to_string()
    }

    /// Expand and return the filesystem path.
    fn expanded_path(&self) -> String {
        shellexpand::tilde(&self.store_path).to_string()
    }

    /// Load tokens from disk. Handles v1 to v2 migration.
    pub(crate) async fn load(&self) -> Result<()> {
        let expanded = self.expanded_path();
        let path = Path::new(&expanded);

        if !path.exists() {
            debug!("No token store at {}, starting fresh", expanded);
            return Ok(());
        }

        let contents = tokio::fs::read_to_string(path).await?;

        if let Ok(v2) = serde_json::from_str::<TokenStoreV2>(&contents)
            && v2.version == 2
        {
            let count = v2.tokens.len();
            let mut store = self.store.write().await;
            *store = v2;
            info!("Loaded {} tokens from v2 store at {}", count, expanded);
            return Ok(());
        }

        if let Ok(v1_map) =
            serde_json::from_str::<std::collections::HashMap<String, TokenEntryV1>>(&contents)
        {
            info!(
                "Detected v1 token store with {} entries, migrating to v2",
                v1_map.len()
            );

            let backup_path = format!("{}.v1.bak", expanded);
            tokio::fs::copy(&expanded, &backup_path).await?;
            info!("Backed up v1 store to {}", backup_path);

            let argon2 = argon2id();
            let mut migrated = Vec::new();
            for (namespace, entry) in &v1_map {
                let salt = SaltString::generate(&mut OsRng);
                let token_hash = argon2
                    .hash_password(entry.token.as_bytes(), &salt)
                    .map_err(|err| anyhow!("Failed to hash v1 token for '{}': {}", namespace, err))?
                    .to_string();

                migrated.push(TokenEntry {
                    id: format!("migrated-{}", namespace),
                    token_hash,
                    scopes: vec![
                        Scope::ContextRead,
                        Scope::ToolExecute,
                        Scope::CliFull,
                        Scope::Admin,
                    ],
                    namespaces: vec![namespace.clone()],
                    expires_at: None,
                    description: entry.description.clone().unwrap_or_else(|| {
                        format!("Migrated from v1 for namespace '{}'", namespace)
                    }),
                    created_at: DateTime::from_timestamp(entry.created_at as i64, 0)
                        .unwrap_or_else(Utc::now),
                });
            }

            {
                let mut store = self.store.write().await;
                *store = TokenStoreV2 {
                    version: 2,
                    tokens: migrated,
                };
            }

            self.save().await?;
            warn!(
                "Migrated v1 token store to v2. Old store backed up to {}",
                backup_path
            );
            return Ok(());
        }

        Err(anyhow!(
            "Cannot parse token store at {}. Expected v2 or v1 format.",
            expanded
        ))
    }

    /// Save current store to disk.
    pub(crate) async fn save(&self) -> Result<()> {
        let expanded = self.expanded_path();
        let path = Path::new(&expanded);

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let store = self.store.read().await;
        let contents = serde_json::to_string_pretty(&*store)?;
        tokio::fs::write(path, contents).await?;
        debug!("Saved {} tokens to {}", store.tokens.len(), expanded);
        Ok(())
    }

    /// Create a new token, hash it, store it, and return the plaintext.
    pub(crate) async fn create_token(
        &self,
        id: String,
        scopes: Vec<Scope>,
        namespaces: Vec<String>,
        expires_at: Option<DateTime<Utc>>,
        description: String,
    ) -> Result<String> {
        {
            let store = self.store.read().await;
            if store.tokens.iter().any(|token| token.id == id) {
                return Err(anyhow!(
                    "Token with id '{}' already exists. Revoke it first or pick a different id.",
                    id
                ));
            }
        }

        let plaintext = format!("loct_{}", Uuid::new_v4().to_string().replace('-', ""));
        let argon2 = argon2id();
        let salt = SaltString::generate(&mut OsRng);
        let token_hash = argon2
            .hash_password(plaintext.as_bytes(), &salt)
            .map_err(|err| anyhow!("Failed to hash token: {}", err))?
            .to_string();

        let entry = TokenEntry {
            id: id.clone(),
            token_hash,
            scopes,
            namespaces,
            expires_at,
            description,
            created_at: Utc::now(),
        };

        {
            let mut store = self.store.write().await;
            store.tokens.push(entry);
        }

        self.save().await?;
        info!("Created token '{}'", id);
        Ok(plaintext)
    }

    /// List all token entries. Plaintext is never exposed.
    pub(crate) async fn list_tokens(&self) -> Vec<TokenEntry> {
        self.store.read().await.tokens.clone()
    }

    /// Revoke a token by id.
    pub(crate) async fn revoke_token(&self, id: &str) -> Result<bool> {
        let removed = {
            let mut store = self.store.write().await;
            let before = store.tokens.len();
            store.tokens.retain(|token| token.id != id);
            store.tokens.len() < before
        };

        if removed {
            self.save().await?;
            info!("Revoked token '{}'", id);
        }
        Ok(removed)
    }

    /// Rotate a token: revoke old, create new with the same metadata.
    pub(crate) async fn rotate_token(&self, id: &str) -> Result<String> {
        let old_entry = {
            let store = self.store.read().await;
            store
                .tokens
                .iter()
                .find(|token| token.id == id)
                .cloned()
                .ok_or_else(|| anyhow!("Token '{}' not found", id))?
        };

        {
            let mut store = self.store.write().await;
            store.tokens.retain(|token| token.id != id);
        }

        self.create_token(
            old_entry.id,
            old_entry.scopes,
            old_entry.namespaces,
            old_entry.expires_at,
            old_entry.description,
        )
        .await
    }

    /// Look up a token by verifying plaintext against every stored hash.
    pub(crate) async fn lookup_by_plaintext(&self, plaintext: &str) -> Option<TokenEntry> {
        let store = self.store.read().await;
        let argon2 = argon2id();

        for entry in &store.tokens {
            if let Ok(parsed_hash) = PasswordHash::new(&entry.token_hash)
                && argon2
                    .verify_password(plaintext.as_bytes(), &parsed_hash)
                    .is_ok()
            {
                return Some(entry.clone());
            }
        }

        None
    }
}

/// Unified auth manager for loctree MCP bearer token checks.
#[derive(Debug)]
pub(crate) struct AuthManager {
    token_store: TokenStoreFile,
    /// Legacy fallback: a single token with wildcard admin access.
    legacy_token: Option<String>,
}

/// Result of authenticating and authorizing a request.
#[derive(Debug, Clone)]
pub(crate) struct AuthResult {
    /// The token entry that authenticated the request.
    pub token: TokenEntry,
}

/// Reason an auth check was denied.
#[derive(Debug, Clone)]
pub(crate) enum AuthDenial {
    /// No bearer token was provided.
    MissingToken,
    /// Token was provided but not recognized.
    InvalidToken,
    /// Token is expired.
    Expired { id: String },
    /// Token lacks the required scope.
    InsufficientScope {
        id: String,
        required: Scope,
        granted: Vec<Scope>,
    },
    /// Token lacks access to the requested namespace.
    NamespaceDenied {
        id: String,
        requested: String,
        allowed: Vec<String>,
    },
}

impl fmt::Display for AuthDenial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthDenial::MissingToken => write!(f, "Authorization header missing or malformed"),
            AuthDenial::InvalidToken => write!(f, "Invalid or unrecognized token"),
            AuthDenial::Expired { id } => write!(f, "Token '{}' has expired", id),
            AuthDenial::InsufficientScope {
                id,
                required,
                granted,
            } => {
                let granted = granted
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "Token '{}' lacks scope '{}' (has: [{}])",
                    id, required, granted
                )
            }
            AuthDenial::NamespaceDenied {
                id,
                requested,
                allowed,
            } => write!(
                f,
                "Token '{}' cannot access namespace '{}' (allowed: [{}])",
                id,
                requested,
                allowed.join(", ")
            ),
        }
    }
}

impl AuthManager {
    /// Create a new AuthManager with the given store path and optional legacy token.
    pub(crate) fn new(store_path: String, legacy_token: Option<String>) -> Self {
        let store_path = if store_path.trim().is_empty() {
            TokenStoreFile::default_store_path()
        } else {
            store_path
        };
        Self {
            token_store: TokenStoreFile::new(store_path),
            legacy_token,
        }
    }

    /// Create an AuthManager using loctree-mcp's default token store path.
    pub(crate) fn with_default_store(legacy_token: Option<String>) -> Self {
        Self::new(TokenStoreFile::default_store_path(), legacy_token)
    }

    /// Initialize: load tokens from disk and warn about legacy token usage.
    pub(crate) async fn init(&self) -> Result<()> {
        self.token_store.load().await?;

        if self.legacy_token.is_some() {
            warn!(
                "DEPRECATED: legacy auth token used. This maps to a single wildcard token. \
                 Migrate to loctree-mcp token store entries for per-token scopes and namespace ACLs."
            );
        }

        Ok(())
    }

    /// Authenticate a bearer token. Scope and namespace are checked by `verify`.
    pub(crate) async fn authenticate(&self, bearer_token: &str) -> Result<AuthResult, AuthDenial> {
        if bearer_token.is_empty() {
            return Err(AuthDenial::MissingToken);
        }

        if let Some(legacy) = &self.legacy_token {
            let same_len = legacy.len() == bearer_token.len();
            let same_value = legacy.as_bytes().ct_eq(bearer_token.as_bytes()).into();
            if same_len && same_value {
                return Ok(AuthResult {
                    token: TokenEntry {
                        id: "__legacy__".to_string(),
                        token_hash: String::new(),
                        scopes: vec![Scope::Admin],
                        namespaces: vec!["*".to_string()],
                        expires_at: None,
                        description: "Legacy auth token (wildcard)".to_string(),
                        created_at: Utc::now(),
                    },
                });
            }
        }

        match self.token_store.lookup_by_plaintext(bearer_token).await {
            Some(entry) => {
                if entry.is_expired() {
                    return Err(AuthDenial::Expired {
                        id: entry.id.clone(),
                    });
                }
                Ok(AuthResult { token: entry })
            }
            None => Err(AuthDenial::InvalidToken),
        }
    }

    /// Full authorization check: authenticate + scope + namespace ACL.
    pub(crate) async fn verify(
        &self,
        bearer_token: &str,
        required_scope: &Scope,
        namespace: &str,
    ) -> Result<AuthResult, AuthDenial> {
        let result = self.authenticate(bearer_token).await?;

        if !result.token.has_scope(required_scope) {
            return Err(AuthDenial::InsufficientScope {
                id: result.token.id.clone(),
                required: required_scope.clone(),
                granted: result.token.scopes.clone(),
            });
        }

        if !result.token.has_namespace_access(namespace) {
            return Err(AuthDenial::NamespaceDenied {
                id: result.token.id.clone(),
                requested: namespace.to_string(),
                allowed: result.token.namespaces.clone(),
            });
        }

        Ok(result)
    }

    /// Backward-compatible alias for callers ported from rust-memex.
    pub(crate) async fn authorize(
        &self,
        bearer_token: &str,
        required_scope: &Scope,
        namespace: Option<&str>,
    ) -> Result<AuthResult, AuthDenial> {
        match namespace {
            Some(namespace) => self.verify(bearer_token, required_scope, namespace).await,
            None => {
                let result = self.authenticate(bearer_token).await?;
                if result.token.has_scope(required_scope) {
                    Ok(result)
                } else {
                    Err(AuthDenial::InsufficientScope {
                        id: result.token.id.clone(),
                        required: required_scope.clone(),
                        granted: result.token.scopes.clone(),
                    })
                }
            }
        }
    }

    /// Delegate to token store: create a new token.
    pub(crate) async fn create_token(
        &self,
        id: String,
        scopes: Vec<Scope>,
        namespaces: Vec<String>,
        expires_at: Option<DateTime<Utc>>,
        description: String,
    ) -> Result<String> {
        self.token_store
            .create_token(id, scopes, namespaces, expires_at, description)
            .await
    }

    /// Delegate to token store: list tokens.
    pub(crate) async fn list_tokens(&self) -> Vec<TokenEntry> {
        self.token_store.list_tokens().await
    }

    /// Delegate to token store: revoke a token.
    pub(crate) async fn revoke_token(&self, id: &str) -> Result<bool> {
        self.token_store.revoke_token(id).await
    }

    /// Delegate to token store: rotate a token.
    pub(crate) async fn rotate_token(&self, id: &str) -> Result<String> {
        self.token_store.rotate_token(id).await
    }

    /// Check if any tokens are configured.
    pub(crate) async fn has_any_tokens(&self) -> bool {
        self.legacy_token.is_some() || !self.token_store.list_tokens().await.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_entry(scopes: Vec<Scope>, namespaces: Vec<&str>) -> TokenEntry {
        TokenEntry {
            id: "test".to_string(),
            token_hash: String::new(),
            scopes,
            namespaces: namespaces.into_iter().map(ToString::to_string).collect(),
            expires_at: None,
            description: "test".to_string(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn token_entry_scope_check() {
        let entry = token_entry(vec![Scope::ContextRead], vec!["repo-a"]);

        assert!(entry.has_scope(&Scope::ContextRead));
        assert!(!entry.has_scope(&Scope::ToolExecute));
        assert!(!entry.has_scope(&Scope::CliFull));
        assert!(!entry.has_scope(&Scope::Admin));
    }

    #[test]
    fn admin_scope_implies_all() {
        let entry = token_entry(vec![Scope::Admin], vec!["*"]);

        assert!(entry.has_scope(&Scope::ContextRead));
        assert!(entry.has_scope(&Scope::ToolExecute));
        assert!(entry.has_scope(&Scope::CliFull));
        assert!(entry.has_scope(&Scope::Admin));
    }

    #[test]
    fn namespace_wildcard_access() {
        let entry = token_entry(vec![Scope::ContextRead], vec!["*"]);

        assert!(entry.has_namespace_access("loctree"));
        assert!(entry.has_namespace_access("customer-a"));
    }

    #[test]
    fn namespace_acl_check() {
        let entry = token_entry(vec![Scope::ContextRead], vec!["loctree", "customer-a"]);

        assert!(entry.has_namespace_access("loctree"));
        assert!(entry.has_namespace_access("customer-a"));
        assert!(!entry.has_namespace_access("customer-b"));
    }

    #[test]
    fn default_store_path_targets_loctree_mcp() {
        assert_eq!(
            TokenStoreFile::default_store_path(),
            "~/.rmcp-servers/loctree-mcp/tokens.json"
        );
        assert_eq!(
            AuthManager::with_default_store(None).token_store.store_path,
            "~/.rmcp-servers/loctree-mcp/tokens.json"
        );
    }

    #[tokio::test]
    async fn token_verify_happy_path() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("tokens.json").display().to_string();

        let manager = AuthManager::new(store_path, None);
        manager.init().await.unwrap();

        let plaintext = manager
            .create_token(
                "context-reader".to_string(),
                vec![Scope::ContextRead],
                vec!["loctree".to_string()],
                None,
                "context token".to_string(),
            )
            .await
            .unwrap();

        assert!(plaintext.starts_with("loct_"));

        let result = manager
            .verify(&plaintext, &Scope::ContextRead, "loctree")
            .await
            .unwrap();
        assert_eq!(result.token.id, "context-reader");

        let stored = manager.list_tokens().await;
        assert_eq!(stored.len(), 1);
        assert!(stored[0].token_hash.starts_with("$argon2id$"));
        assert_ne!(stored[0].token_hash, plaintext);
    }

    #[tokio::test]
    async fn namespace_acl_deny() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("tokens.json").display().to_string();

        let manager = AuthManager::new(store_path, None);
        let plaintext = manager
            .create_token(
                "limited".to_string(),
                vec![Scope::ContextRead],
                vec!["customer-a".to_string()],
                None,
                "limited token".to_string(),
            )
            .await
            .unwrap();

        let denied = manager
            .verify(&plaintext, &Scope::ContextRead, "customer-b")
            .await
            .unwrap_err();

        match denied {
            AuthDenial::NamespaceDenied {
                requested, allowed, ..
            } => {
                assert_eq!(requested, "customer-b");
                assert_eq!(allowed, vec!["customer-a".to_string()]);
            }
            other => panic!("Expected NamespaceDenied, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn scope_mismatch_deny() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("tokens.json").display().to_string();

        let manager = AuthManager::new(store_path, None);
        let plaintext = manager
            .create_token(
                "read-only".to_string(),
                vec![Scope::ContextRead],
                vec!["*".to_string()],
                None,
                "read-only token".to_string(),
            )
            .await
            .unwrap();

        let denied = manager
            .verify(&plaintext, &Scope::ToolExecute, "loctree")
            .await
            .unwrap_err();

        match denied {
            AuthDenial::InsufficientScope {
                required, granted, ..
            } => {
                assert_eq!(required, Scope::ToolExecute);
                assert_eq!(granted, vec![Scope::ContextRead]);
            }
            other => panic!("Expected InsufficientScope, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn legacy_token_has_wildcard_admin_access() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("tokens.json").display().to_string();

        let manager = AuthManager::new(store_path, Some("legacy-token".to_string()));
        let result = manager
            .verify("legacy-token", &Scope::CliFull, "any-namespace")
            .await
            .unwrap();

        assert_eq!(result.token.id, "__legacy__");
        assert!(result.token.has_scope(&Scope::Admin));
        assert!(result.token.has_namespace_access("anything"));
    }

    #[tokio::test]
    async fn token_revoke_rotate_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("tokens.json").display().to_string();

        let store = TokenStoreFile::new(store_path.clone());
        let old_plaintext = store
            .create_token(
                "rotate-me".to_string(),
                vec![Scope::ContextRead],
                vec!["loctree".to_string()],
                None,
                "rotate test".to_string(),
            )
            .await
            .unwrap();

        let persisted = TokenStoreFile::new(store_path.clone());
        persisted.load().await.unwrap();
        assert!(
            persisted
                .lookup_by_plaintext(&old_plaintext)
                .await
                .is_some()
        );

        let new_plaintext = persisted.rotate_token("rotate-me").await.unwrap();
        assert_ne!(old_plaintext, new_plaintext);
        assert!(
            persisted
                .lookup_by_plaintext(&old_plaintext)
                .await
                .is_none()
        );
        assert!(
            persisted
                .lookup_by_plaintext(&new_plaintext)
                .await
                .is_some()
        );

        assert!(persisted.revoke_token("rotate-me").await.unwrap());
        assert!(
            persisted
                .lookup_by_plaintext(&new_plaintext)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn v1_migration() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("tokens.json");

        let v1_data: std::collections::HashMap<String, serde_json::Value> = [(
            "loctree".to_string(),
            serde_json::json!({
                "namespace": "loctree",
                "token": "legacy_plaintext",
                "created_at": 1700000000_u64,
                "description": "Original v1 token"
            }),
        )]
        .into_iter()
        .collect();

        tokio::fs::write(&store_path, serde_json::to_string_pretty(&v1_data).unwrap())
            .await
            .unwrap();

        let store = TokenStoreFile::new(store_path.display().to_string());
        store.load().await.unwrap();

        let tokens = store.list_tokens().await;
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].id, "migrated-loctree");
        assert_eq!(tokens[0].namespaces, vec!["loctree".to_string()]);
        assert_eq!(
            tokens[0].scopes,
            vec![
                Scope::ContextRead,
                Scope::ToolExecute,
                Scope::CliFull,
                Scope::Admin
            ]
        );
        assert!(tokens[0].token_hash.starts_with("$argon2id$"));
        assert!(
            store
                .lookup_by_plaintext("legacy_plaintext")
                .await
                .is_some()
        );

        let backup_path = format!("{}.v1.bak", store_path.display());
        assert!(Path::new(&backup_path).exists());
    }
}
