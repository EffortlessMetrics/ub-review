//! Offline release-byte contracts (#1343) and the production publish-path
//! adapter (#1344). The adapter admits exact authorized bytes and creates a
//! release once; it has no overwrite or delete capability.
pub mod install_proof;
pub mod release_bytes;
pub mod release_publish;
