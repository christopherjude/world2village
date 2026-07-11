//! Invite codes: a compact, pasteable string encoding of a [`ServerProfile`]'s
//! shareable fields.
//!
//! Format: `VLG1-<base64url_nopad(json_bytes ++ crc32_le_bytes)>` where
//! `json_bytes` is the JSON serialization of an [`InvitePayload`]. The CRC32
//! suffix exists purely to turn "user garbled the paste" into a friendly
//! [`InviteError::Corrupted`] instead of a confusing JSON parse error deep
//! in the decode path.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::profile::{
    AdvancedSettings, Community, PassKey, ProfileError, ServerProfile, SupernodeAddr,
};

const PREFIX: &str = "VLG";
const VERSION: u8 = 1;

/// The shareable subset of a [`ServerProfile`]'s fields — no `id`, since a
/// fresh one is assigned locally on import.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct InvitePayload {
    nickname: String,
    community: String,
    key: String,
    supernode: String,
    advanced: AdvancedSettings,
}

impl From<&ServerProfile> for InvitePayload {
    fn from(profile: &ServerProfile) -> Self {
        Self {
            nickname: profile.nickname.clone(),
            community: profile.community.as_str().to_string(),
            key: profile.key.as_str().to_string(),
            supernode: profile.supernode.to_string(),
            advanced: profile.advanced.clone(),
        }
    }
}

/// Errors returned by [`decode_invite`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InviteError {
    #[error("this doesn't look like a Village invite code")]
    UnrecognizedPrefix,
    #[error("this invite code was created by a newer version of Village (version {0})")]
    UnsupportedVersion(u8),
    #[error("this invite code is corrupted or was pasted incompletely")]
    Corrupted,
    #[error("invite code contains an invalid value: {0}")]
    Invalid(#[from] ProfileError),
}

/// Encode a profile's shareable fields as a `VLG1-...` invite code.
pub fn encode_invite(profile: &ServerProfile) -> String {
    let payload = InvitePayload::from(profile);
    // Serializing our own well-formed InvitePayload can't fail.
    let json_bytes = serde_json::to_vec(&payload).expect("InvitePayload always serializes");

    let checksum = crc32fast::hash(&json_bytes);

    let mut body = json_bytes;
    body.extend_from_slice(&checksum.to_le_bytes());

    let encoded = URL_SAFE_NO_PAD.encode(body);
    format!("{PREFIX}{VERSION}-{encoded}")
}

/// Decode a `VLG1-...` invite code back into a fresh [`ServerProfile`]
/// (with a newly generated `id`), re-running the same field validation
/// used for manual entry.
pub fn decode_invite(code: &str) -> Result<ServerProfile, InviteError> {
    let rest = code.strip_prefix(PREFIX).ok_or(InviteError::UnrecognizedPrefix)?;

    // Expect a single version digit immediately after the prefix, followed
    // by '-'. Anything else (missing digit, missing separator, non-numeric)
    // means this isn't a recognizable Village invite code at all.
    let mut chars = rest.chars();
    let version_char = chars.next().ok_or(InviteError::UnrecognizedPrefix)?;
    let version: u8 = version_char
        .to_digit(10)
        .ok_or(InviteError::UnrecognizedPrefix)? as u8;
    let remainder = chars.as_str();
    let encoded = remainder
        .strip_prefix('-')
        .ok_or(InviteError::UnrecognizedPrefix)?;

    if version != VERSION {
        return Err(InviteError::UnsupportedVersion(version));
    }

    let body = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| InviteError::Corrupted)?;

    if body.len() < 4 {
        return Err(InviteError::Corrupted);
    }
    let (json_bytes, checksum_bytes) = body.split_at(body.len() - 4);
    let expected_checksum = u32::from_le_bytes(
        checksum_bytes
            .try_into()
            .expect("split_at(len - 4) always yields a 4-byte suffix"),
    );
    let actual_checksum = crc32fast::hash(json_bytes);
    if actual_checksum != expected_checksum {
        return Err(InviteError::Corrupted);
    }

    let payload: InvitePayload =
        serde_json::from_slice(json_bytes).map_err(|_| InviteError::Corrupted)?;

    let community = Community::new(payload.community)?;
    let key = PassKey::new(payload.key)?;
    let supernode = SupernodeAddr::new(payload.supernode)?;

    Ok(ServerProfile {
        id: Uuid::new_v4(),
        nickname: payload.nickname,
        community,
        key,
        supernode,
        advanced: payload.advanced,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Cipher, Compression};

    fn sample_profile() -> ServerProfile {
        ServerProfile {
            id: Uuid::new_v4(),
            nickname: "Friend's LAN".to_string(),
            community: Community::new("generals").unwrap(),
            key: PassKey::new("supersecret").unwrap(),
            supernode: SupernodeAddr::new("sn.example.com:7654").unwrap(),
            advanced: AdvancedSettings {
                mtu: Some(1400),
                header_encryption: true,
                cipher: Some(Cipher::Aes),
                compression: Some(Compression::Zstd),
            },
        }
    }

    #[test]
    fn round_trip_preserves_fields() {
        let profile = sample_profile();
        let code = encode_invite(&profile);
        assert!(code.starts_with("VLG1-"));

        let decoded = decode_invite(&code).unwrap();
        assert_eq!(decoded.nickname, profile.nickname);
        assert_eq!(decoded.community, profile.community);
        assert_eq!(decoded.key, profile.key);
        assert_eq!(decoded.supernode, profile.supernode);
        assert_eq!(decoded.advanced, profile.advanced);
        // id is freshly assigned, not copied from the original.
        assert_ne!(decoded.id, profile.id);
    }

    #[test]
    fn tampered_body_char_is_corrupted_not_panic() {
        let code = encode_invite(&sample_profile());
        let mut chars: Vec<char> = code.chars().collect();
        // Flip a character somewhere in the base64 body (well past the
        // "VLG1-" prefix) to something different.
        let idx = chars.len() - 5;
        chars[idx] = if chars[idx] == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();

        assert_eq!(decode_invite(&tampered), Err(InviteError::Corrupted));
    }

    #[test]
    fn truncated_code_is_corrupted_not_panic() {
        let code = encode_invite(&sample_profile());
        let truncated = &code[..code.len() - 10];
        assert_eq!(decode_invite(truncated), Err(InviteError::Corrupted));
    }

    #[test]
    fn bogus_prefix_is_unrecognized() {
        assert_eq!(
            decode_invite("NOTVLG-abc123"),
            Err(InviteError::UnrecognizedPrefix)
        );
    }

    #[test]
    fn missing_separator_is_unrecognized() {
        assert_eq!(
            decode_invite("VLG1abc123"),
            Err(InviteError::UnrecognizedPrefix)
        );
    }

    #[test]
    fn higher_version_is_unsupported() {
        let code = encode_invite(&sample_profile());
        let bumped = code.replacen("VLG1-", "VLG9-", 1);
        assert_eq!(decode_invite(&bumped), Err(InviteError::UnsupportedVersion(9)));
    }
}
