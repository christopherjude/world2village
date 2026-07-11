//! Validated data model for a saved n2n "server" profile.
//!
//! Deliberately absent: any field representing n2n's `-a` IP-address-mode
//! flag. See `CLAUDE.md`'s "`-a dhcp` is a trap" gotcha — Village always
//! lets the supernode assign the overlay IP (i.e. `edge.exe` is invoked
//! with `-a` omitted entirely), so there is no data representation of an
//! IP mode anywhere in this crate for a future caller to accidentally wire
//! up into an argv builder.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// n2n's `N2N_COMMUNITY_SIZE` is 20 bytes including a NUL terminator, so at
/// most 19 usable ASCII bytes are available for the community name.
const COMMUNITY_MAX_LEN: usize = 19;

const PASSKEY_MAX_LEN: usize = 128;

/// Errors returned by the validated newtype constructors in this module.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("community name must not be empty")]
    CommunityEmpty,
    #[error("community name must be {COMMUNITY_MAX_LEN} ASCII bytes or fewer")]
    CommunityTooLong,
    #[error("community name must be ASCII")]
    CommunityNotAscii,
    #[error("community name must not start with '-' or contain whitespace/control characters")]
    CommunityUnsafe,

    #[error("key must not be empty or whitespace-only")]
    KeyEmpty,
    #[error("key must be {PASSKEY_MAX_LEN} characters or fewer")]
    KeyTooLong,
    #[error("key must consist of printable ASCII characters")]
    KeyNotPrintableAscii,
    #[error("key must not start with '-' or contain whitespace/control characters")]
    KeyUnsafe,

    #[error("supernode address must not be empty")]
    SupernodeEmpty,
    #[error("supernode address must be in host:port form")]
    SupernodeMalformed,
    #[error("supernode host must not be empty")]
    SupernodeHostEmpty,
    #[error("supernode port must be between 1 and 65535")]
    SupernodePortInvalid,
    #[error("supernode address must not start with '-' or contain whitespace/control characters")]
    SupernodeUnsafe,

    #[error("nickname must not be empty")]
    NicknameEmpty,
}

/// Shared "does this look safe to hand to `edge.exe`'s argv" check: reject
/// values that start with `-` (so they can't be misread as a flag by
/// edge's own getopt) or that contain any whitespace/control character
/// (so a single argv element can't smuggle in extra structure).
fn reject_unsafe_argv_value(value: &str) -> bool {
    value.starts_with('-') || value.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// A validated n2n community name: ASCII, 1-19 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Community(String);

impl Community {
    pub fn new(value: impl Into<String>) -> Result<Self, ProfileError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ProfileError::CommunityEmpty);
        }
        if !value.is_ascii() {
            return Err(ProfileError::CommunityNotAscii);
        }
        if value.len() > COMMUNITY_MAX_LEN {
            return Err(ProfileError::CommunityTooLong);
        }
        if reject_unsafe_argv_value(&value) {
            return Err(ProfileError::CommunityUnsafe);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for Community {
    type Error = ProfileError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl std::fmt::Display for Community {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A validated n2n community passphrase: printable ASCII, 1-128 chars.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassKey(String);

impl PassKey {
    pub fn new(value: impl Into<String>) -> Result<Self, ProfileError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ProfileError::KeyEmpty);
        }
        if value.len() > PASSKEY_MAX_LEN {
            return Err(ProfileError::KeyTooLong);
        }
        if reject_unsafe_argv_value(&value) {
            return Err(ProfileError::KeyUnsafe);
        }
        if !value.chars().all(|c| c.is_ascii_graphic()) {
            return Err(ProfileError::KeyNotPrintableAscii);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for PassKey {
    type Error = ProfileError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

// Deliberately no Display/Debug that prints the raw key — avoid it leaking
// into logs by accident. Debug is hand-rolled to redact the value.
impl std::fmt::Debug for PassKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("PassKey").field(&"<redacted>").finish()
    }
}

/// A validated `host:port` supernode address.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SupernodeAddr {
    host: String,
    port: u16,
}

impl SupernodeAddr {
    pub fn new(value: impl AsRef<str>) -> Result<Self, ProfileError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(ProfileError::SupernodeEmpty);
        }
        if reject_unsafe_argv_value(value) {
            return Err(ProfileError::SupernodeUnsafe);
        }
        let (host, port_str) = value
            .rsplit_once(':')
            .ok_or(ProfileError::SupernodeMalformed)?;
        if host.is_empty() {
            return Err(ProfileError::SupernodeHostEmpty);
        }
        let port: u16 = port_str
            .parse()
            .map_err(|_| ProfileError::SupernodePortInvalid)?;
        if port == 0 {
            return Err(ProfileError::SupernodePortInvalid);
        }
        Ok(Self {
            host: host.to_string(),
            port,
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl TryFrom<&str> for SupernodeAddr {
    type Error = ProfileError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl std::fmt::Display for SupernodeAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

/// n2n cipher selector, matching `edge.exe`'s `-A<n>` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cipher {
    None,
    Twofish,
    Aes,
    ChaCha20,
    Speck,
}

impl Cipher {
    pub fn code(&self) -> u8 {
        match self {
            Cipher::None => 1,
            Cipher::Twofish => 2,
            Cipher::Aes => 3,
            Cipher::ChaCha20 => 4,
            Cipher::Speck => 5,
        }
    }
}

/// n2n compression selector, matching `edge.exe`'s `-z<n>` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Compression {
    Lzo1x,
    Zstd,
}

impl Compression {
    pub fn code(&self) -> u8 {
        match self {
            Compression::Lzo1x => 1,
            Compression::Zstd => 2,
        }
    }
}

/// Optional, collapsed-by-default advanced settings.
///
/// Deliberately has no IP-address-mode field — see the module doc comment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvancedSettings {
    pub mtu: Option<u16>,
    pub header_encryption: bool,
    pub cipher: Option<Cipher>,
    pub compression: Option<Compression>,
}

/// A saved server profile, as stored in the user's config and shown in the
/// server list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerProfile {
    pub id: Uuid,
    pub nickname: String,
    pub community: Community,
    pub key: PassKey,
    pub supernode: SupernodeAddr,
    pub advanced: AdvancedSettings,
}

impl ServerProfile {
    /// Validate a nickname on its own; nicknames are display-only (never
    /// passed to `edge.exe`) so they only need a non-empty check.
    pub fn validate_nickname(nickname: &str) -> Result<(), ProfileError> {
        if nickname.trim().is_empty() {
            return Err(ProfileError::NicknameEmpty);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn community_accepts_valid() {
        assert!(Community::new("gamers").is_ok());
        assert!(Community::new("a".repeat(19)).is_ok());
    }

    #[test]
    fn community_rejects_empty() {
        assert_eq!(Community::new(""), Err(ProfileError::CommunityEmpty));
    }

    #[test]
    fn community_rejects_too_long() {
        assert_eq!(
            Community::new("a".repeat(20)),
            Err(ProfileError::CommunityTooLong)
        );
    }

    #[test]
    fn community_rejects_non_ascii() {
        assert_eq!(Community::new("café"), Err(ProfileError::CommunityNotAscii));
    }

    #[test]
    fn community_rejects_leading_dash() {
        assert_eq!(Community::new("-foo"), Err(ProfileError::CommunityUnsafe));
    }

    #[test]
    fn community_rejects_whitespace() {
        assert_eq!(
            Community::new("foo bar"),
            Err(ProfileError::CommunityUnsafe)
        );
    }

    #[test]
    fn passkey_accepts_valid() {
        assert!(PassKey::new("correcthorsebatterystaple").is_ok());
        assert!(PassKey::new("Sup3r_Secret_Key!42").is_ok());
    }

    #[test]
    fn passkey_rejects_embedded_whitespace() {
        assert_eq!(
            PassKey::new("correct horse battery staple"),
            Err(ProfileError::KeyUnsafe)
        );
    }

    #[test]
    fn passkey_rejects_empty_and_whitespace_only() {
        assert_eq!(PassKey::new(""), Err(ProfileError::KeyEmpty));
        assert_eq!(PassKey::new("   "), Err(ProfileError::KeyEmpty));
    }

    #[test]
    fn passkey_rejects_leading_dash() {
        assert_eq!(PassKey::new("-secret"), Err(ProfileError::KeyUnsafe));
    }

    #[test]
    fn passkey_rejects_control_chars() {
        assert_eq!(PassKey::new("abc\tdef"), Err(ProfileError::KeyUnsafe));
    }

    #[test]
    fn passkey_rejects_non_ascii() {
        assert_eq!(
            PassKey::new("café1234"),
            Err(ProfileError::KeyNotPrintableAscii)
        );
    }

    #[test]
    fn passkey_debug_redacts() {
        let key = PassKey::new("supersecret").unwrap();
        assert_eq!(format!("{key:?}"), "PassKey(\"<redacted>\")");
    }

    #[test]
    fn supernode_parses_host_port() {
        let addr = SupernodeAddr::new("example.com:7654").unwrap();
        assert_eq!(addr.host(), "example.com");
        assert_eq!(addr.port(), 7654);
        assert_eq!(addr.to_string(), "example.com:7654");
    }

    #[test]
    fn supernode_rejects_zero_port() {
        assert_eq!(
            SupernodeAddr::new("example.com:0"),
            Err(ProfileError::SupernodePortInvalid)
        );
    }

    #[test]
    fn supernode_rejects_missing_port() {
        assert_eq!(
            SupernodeAddr::new("example.com"),
            Err(ProfileError::SupernodeMalformed)
        );
    }

    #[test]
    fn supernode_rejects_empty_host() {
        assert_eq!(
            SupernodeAddr::new(":7654"),
            Err(ProfileError::SupernodeHostEmpty)
        );
    }

    #[test]
    fn supernode_rejects_out_of_range_port() {
        assert_eq!(
            SupernodeAddr::new("example.com:70000"),
            Err(ProfileError::SupernodePortInvalid)
        );
    }

    #[test]
    fn supernode_rejects_leading_dash() {
        assert_eq!(
            SupernodeAddr::new("-example.com:7654"),
            Err(ProfileError::SupernodeUnsafe)
        );
    }

    #[test]
    fn cipher_codes() {
        assert_eq!(Cipher::None.code(), 1);
        assert_eq!(Cipher::Twofish.code(), 2);
        assert_eq!(Cipher::Aes.code(), 3);
        assert_eq!(Cipher::ChaCha20.code(), 4);
        assert_eq!(Cipher::Speck.code(), 5);
    }

    #[test]
    fn compression_codes() {
        assert_eq!(Compression::Lzo1x.code(), 1);
        assert_eq!(Compression::Zstd.code(), 2);
    }
}
