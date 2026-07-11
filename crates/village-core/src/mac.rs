//! Per-install MAC address generation.
//!
//! Two edges connecting to the same supernode with identical/default MACs
//! collide ("authentication error, MAC or IP address already in use") — see
//! `CLAUDE.md`. Village generates one random, locally-administered MAC per
//! install and persists it in config; it is never hardcoded.

use serde::{Deserialize, Serialize};

/// A 6-byte Ethernet MAC address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MacAddr([u8; 6]);

impl MacAddr {
    pub fn from_bytes(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> [u8; 6] {
        self.0
    }

    /// Generate a MAC from the given RNG, setting the locally-administered
    /// bit and clearing the multicast bit on the first byte so the result
    /// is a valid, non-multicast, locally-administered address.
    pub fn generate(rng: &mut impl rand::Rng) -> MacAddr {
        let mut bytes = [0u8; 6];
        rng.fill_bytes(&mut bytes);
        bytes[0] = (bytes[0] & 0xFE) | 0x02;
        MacAddr(bytes)
    }

    /// Convenience wrapper around [`MacAddr::generate`] using the thread-local RNG.
    pub fn generate_random() -> MacAddr {
        Self::generate(&mut rand::rng())
    }
}

impl std::fmt::Display for MacAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02X}:{b:02X}:{c:02X}:{d:02X}:{e:02X}:{g:02X}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn display_formats_uppercase_colon_separated() {
        let mac = MacAddr::from_bytes([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        assert_eq!(mac.to_string(), "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn generate_sets_locally_administered_clears_multicast() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        for _ in 0..100 {
            let mac = MacAddr::generate(&mut rng);
            let b0 = mac.as_bytes()[0];
            assert_eq!(b0 & 0x02, 0x02, "locally-administered bit must be set");
            assert_eq!(b0 & 0x01, 0x00, "multicast bit must be cleared");
        }
    }

    #[test]
    fn generate_random_produces_valid_mac() {
        let mac = MacAddr::generate_random();
        let b0 = mac.as_bytes()[0];
        assert_eq!(b0 & 0x02, 0x02);
        assert_eq!(b0 & 0x01, 0x00);
    }
}
