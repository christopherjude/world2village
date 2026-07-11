//! Builds the argv passed to `edge.exe`.
//!
//! Returns a `Vec<String>` only — nothing in this crate ever spawns a
//! process or builds a shell string, so there is no shell-interpolation
//! risk; the caller (`village-service`) is expected to hand these elements
//! directly to `std::process::Command::arg()` one at a time.

use crate::mac::MacAddr;
use crate::profile::{AdvancedSettings, Community, PassKey, ServerProfile, SupernodeAddr};

/// The fields actually needed to build `edge.exe`'s argv, borrowed from a
/// [`ServerProfile`] plus the per-install MAC.
pub struct EdgeArgs<'a> {
    pub community: &'a Community,
    pub key: &'a PassKey,
    pub supernode: &'a SupernodeAddr,
    pub mac: MacAddr,
    pub advanced: &'a AdvancedSettings,
}

impl<'a> EdgeArgs<'a> {
    pub fn new(profile: &'a ServerProfile, mac: MacAddr) -> Self {
        Self {
            community: &profile.community,
            key: &profile.key,
            supernode: &profile.supernode,
            mac,
            advanced: &profile.advanced,
        }
    }
}

impl<'a> From<(&'a ServerProfile, MacAddr)> for EdgeArgs<'a> {
    fn from((profile, mac): (&'a ServerProfile, MacAddr)) -> Self {
        Self::new(profile, mac)
    }
}

/// Builds the full `edge.exe` argv for a given profile/MAC/advanced
/// settings combination.
///
/// Always emits `-c <community> -k <key> -l <supernode> -m <mac>`, then
/// `-M <mtu>`, `-H`, `-A<n>`, `-z<n>` if the corresponding advanced setting
/// is set.
///
/// # `-a` is never emitted
///
/// Per `CLAUDE.md`'s "`-a dhcp` is a trap" gotcha: `-a dhcp` does NOT mean
/// "ask the supernode for an IP" — it selects n2n's
/// `TUNTAP_IP_MODE_DHCP`, which waits forever for another peer on the
/// overlay to run a real DHCP server. Omitting `-a` entirely selects
/// `TUNTAP_IP_MODE_SN_ASSIGN` (supernode-assigned IP), which is what
/// Village always wants. This function has no code path capable of
/// emitting `-a` under any circumstance — there isn't even a data field to
/// wire one up from (see `profile::AdvancedSettings`).
pub fn build_edge_argv(args: &EdgeArgs) -> Vec<String> {
    let mut argv = vec![
        "-c".to_string(),
        args.community.as_str().to_string(),
        "-k".to_string(),
        args.key.as_str().to_string(),
        "-l".to_string(),
        args.supernode.to_string(),
        "-m".to_string(),
        args.mac.to_string(),
    ];

    if let Some(mtu) = args.advanced.mtu {
        argv.push("-M".to_string());
        argv.push(mtu.to_string());
    }

    if args.advanced.header_encryption {
        argv.push("-H".to_string());
    }

    if let Some(cipher) = &args.advanced.cipher {
        argv.push(format!("-A{}", cipher.code()));
    }

    if let Some(compression) = &args.advanced.compression {
        argv.push(format!("-z{}", compression.code()));
    }

    argv
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Cipher, Compression};
    use uuid::Uuid;

    fn minimal_profile() -> ServerProfile {
        ServerProfile {
            id: Uuid::nil(),
            nickname: "Test Server".to_string(),
            community: Community::new("gamers").unwrap(),
            key: PassKey::new("supersecret").unwrap(),
            supernode: SupernodeAddr::new("sn.example.com:7654").unwrap(),
            advanced: AdvancedSettings::default(),
        }
    }

    fn full_advanced_profile() -> ServerProfile {
        ServerProfile {
            id: Uuid::nil(),
            nickname: "Full Advanced Server".to_string(),
            community: Community::new("generals").unwrap(),
            key: PassKey::new("anothersecret").unwrap(),
            supernode: SupernodeAddr::new("10.0.0.1:9001").unwrap(),
            advanced: AdvancedSettings {
                mtu: Some(1400),
                header_encryption: true,
                cipher: Some(Cipher::Aes),
                compression: Some(Compression::Zstd),
            },
        }
    }

    fn test_mac() -> MacAddr {
        MacAddr::from_bytes([0x02, 0x11, 0x22, 0x33, 0x44, 0x55])
    }

    fn all_test_argvs() -> Vec<Vec<String>> {
        vec![
            build_edge_argv(&EdgeArgs::new(&minimal_profile(), test_mac())),
            build_edge_argv(&EdgeArgs::new(&full_advanced_profile(), test_mac())),
        ]
    }

    #[test]
    fn never_emits_dash_a_flag() {
        for argv in all_test_argvs() {
            assert!(
                !argv.contains(&"-a".to_string()),
                "argv must never contain a bare -a flag: {argv:?}"
            );
            for elem in &argv {
                assert!(
                    !(elem.starts_with("-a") && elem.len() > 1),
                    "argv must never contain a concatenated -a-prefixed element: {argv:?}"
                );
            }
        }
    }

    #[test]
    fn golden_minimal_profile() {
        let argv = build_edge_argv(&EdgeArgs::new(&minimal_profile(), test_mac()));
        assert_eq!(
            argv,
            vec![
                "-c", "gamers", "-k", "supersecret", "-l", "sn.example.com:7654", "-m",
                "02:11:22:33:44:55",
            ]
        );
    }

    #[test]
    fn golden_full_advanced_profile() {
        let argv = build_edge_argv(&EdgeArgs::new(&full_advanced_profile(), test_mac()));
        assert_eq!(
            argv,
            vec![
                "-c",
                "generals",
                "-k",
                "anothersecret",
                "-l",
                "10.0.0.1:9001",
                "-m",
                "02:11:22:33:44:55",
                "-M",
                "1400",
                "-H",
                "-A3",
                "-z2",
            ]
        );
    }

    #[test]
    fn golden_header_encryption_only() {
        let mut profile = minimal_profile();
        profile.advanced.header_encryption = true;
        let argv = build_edge_argv(&EdgeArgs::new(&profile, test_mac()));
        assert_eq!(argv.last().unwrap(), "-H");
        assert!(!argv.contains(&"-M".to_string()));
    }
}
