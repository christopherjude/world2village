//! Wire protocol exchanged between the GUI (unelevated) and
//! `village-service` (SYSTEM-elevated) over the named pipe.
//!
//! This crate deliberately does not depend on `village-core`: the newtypes
//! that validate profile fields live there, and the service re-validates
//! every incoming [`ResolvedProfile`] through those constructors before
//! acting on it — this crate only defines the plain-data wire shapes. Both
//! `Request` and `Response` use `#[serde(deny_unknown_fields)]` to keep the
//! IPC surface a closed, fixed set of operations.

use serde::{Deserialize, Serialize};

/// A profile as it travels over the wire: the same shape as
/// `village-core`'s `ServerProfile`, but as plain strings/primitives. The
/// receiving side (the service) re-parses/re-validates every field through
/// `village-core`'s newtype constructors — this struct itself performs no
/// validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedProfile {
    pub nickname: String,
    pub community: String,
    pub key: String,
    pub supernode: String,
    pub mac: String,
    pub mtu: Option<u16>,
    pub header_encryption: bool,
    pub cipher: Option<u8>,
    pub compression: Option<u8>,
}

/// A request sent from the GUI to the service over the named pipe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", deny_unknown_fields)]
pub enum Request {
    StartProfile { profile: ResolvedProfile },
    Stop,
    Status,
    Ping,
}

/// The connection state reported by [`Response::Status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", deny_unknown_fields)]
pub enum ConnectionStatus {
    Idle,
    Starting,
    Connected {
        overlay_ip: String,
        since_unix_secs: u64,
    },
    Error {
        message: String,
    },
}

/// A closed set of error codes the service can report back to the GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    NotRunning,
    AlreadyRunning,
    InvalidProfile,
    SpawnFailed,
    Internal,
}

/// A response sent from the service back to the GUI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", deny_unknown_fields)]
pub enum Response {
    Ok,
    Status(ConnectionStatus),
    Error { code: ErrorCode, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_profile() -> ResolvedProfile {
        ResolvedProfile {
            nickname: "Test Server".to_string(),
            community: "generals".to_string(),
            key: "supersecret".to_string(),
            supernode: "sn.example.com:7654".to_string(),
            mac: "02:11:22:33:44:55".to_string(),
            mtu: Some(1400),
            header_encryption: true,
            cipher: Some(3),
            compression: Some(2),
        }
    }

    fn round_trip_request(req: &Request) {
        let json = serde_json::to_string(req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, req);
    }

    fn round_trip_response(resp: &Response) {
        let json = serde_json::to_string(resp).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, resp);
    }

    #[test]
    fn request_round_trips_all_variants() {
        round_trip_request(&Request::StartProfile {
            profile: sample_profile(),
        });
        round_trip_request(&Request::Stop);
        round_trip_request(&Request::Status);
        round_trip_request(&Request::Ping);
    }

    #[test]
    fn response_round_trips_all_variants() {
        round_trip_response(&Response::Ok);
        round_trip_response(&Response::Status(ConnectionStatus::Idle));
        round_trip_response(&Response::Status(ConnectionStatus::Starting));
        round_trip_response(&Response::Status(ConnectionStatus::Connected {
            overlay_ip: "10.100.0.5".to_string(),
            since_unix_secs: 1_720_000_000,
        }));
        round_trip_response(&Response::Status(ConnectionStatus::Error {
            message: "supernode unreachable".to_string(),
        }));
        round_trip_response(&Response::Error {
            code: ErrorCode::SpawnFailed,
            message: "could not launch edge.exe".to_string(),
        });
    }

    #[test]
    fn request_rejects_unknown_field() {
        // Note: a unit variant like `Ping` (no fields of its own) can't
        // demonstrate `deny_unknown_fields` here — serde's internally
        // tagged enum support has a known limitation where extra top-level
        // fields are silently ignored when the matched variant is a unit
        // variant (serde-rs/serde#1358). A field-bearing variant like
        // `StartProfile` does correctly reject unknown fields, which is
        // what this test proves `deny_unknown_fields` is active for.
        let json = concat!(
            r#"{"op":"StartProfile","profile":{"nickname":"T","community":"c","#,
            r#""key":"k","supernode":"s:1","mac":"AA:BB:CC:DD:EE:FF","mtu":null,"#,
            r#""header_encryption":false,"cipher":null,"compression":null},"#,
            r#""extra":"nope"}"#
        );
        let result: Result<Request, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn response_rejects_unknown_field() {
        let json = r#"{"result":"Error","code":"Internal","message":"boom","extra":"nope"}"#;
        let result: Result<Response, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
