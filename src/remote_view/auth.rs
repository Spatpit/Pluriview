//! Request authentication that never puts the pairing secret on the wire.
//!
//! Each request carries `Authorization: Pluriview-HMAC t=<ms>, n=<nonce>,
//! m=<mac>`, where the MAC is HMAC-SHA256 over the method, path, client
//! identity, client time and a fresh nonce, keyed by the pairing token. The
//! client time is its own monotonic clock, so no clock agreement is needed:
//! the host only rejects reused nonces and times too far behind the newest it
//! has accepted for that token.
use sha2::{Digest, Sha256};
use std::collections::VecDeque;

pub(super) const SCHEME: &str = "Pluriview-HMAC";
/// How far behind the newest accepted time a request may be. Requests from
/// the phone's parallel video, control, audio and camera connections can
/// arrive slightly out of order.
const REPLAY_WINDOW_MS: u64 = 60_000;
/// Remembered nonces per token. Older entries raise a floor instead.
const MAX_REMEMBERED: usize = 4096;

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|byte| byte ^ 0x36));
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(block.map(|byte| byte ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

pub(super) fn signature(
    token: &str,
    method: &str,
    path: &str,
    client: &str,
    time: u64,
    nonce: &str,
) -> [u8; 32] {
    let message = format!("pluriview-hmac-v1\n{method}\n{path}\n{client}\n{time}\n{nonce}");
    hmac_sha256(token.as_bytes(), message.as_bytes())
}

pub(super) struct Credentials {
    time: u64,
    nonce: String,
    mac: [u8; 32],
}

fn hex_bytes<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != N * 2 || !text.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0u8; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

impl Credentials {
    pub(super) fn parse(header: &str) -> Option<Self> {
        let fields = header.strip_prefix(SCHEME)?.strip_prefix(' ')?;
        let (mut time, mut nonce, mut mac) = (None, None, None);
        for field in fields.split(',') {
            let (name, value) = field.trim().split_once('=')?;
            let slot = match name {
                "t" => &mut time,
                "n" => &mut nonce,
                "m" => &mut mac,
                _ => return None,
            };
            if slot.replace(value).is_some() {
                return None;
            }
        }
        let time = time?;
        if time.is_empty() || time.len() > 19 || !time.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let nonce = nonce?;
        hex_bytes::<16>(nonce)?;
        Some(Self {
            time: time.parse().ok()?,
            nonce: nonce.to_ascii_lowercase(),
            mac: hex_bytes::<32>(mac?)?,
        })
    }

    /// Constant-time comparison against the expected MAC for this token.
    pub(super) fn signed_by(&self, token: &str, method: &str, path: &str, client: &str) -> bool {
        let expected = signature(token, method, path, client, self.time, &self.nonce);
        expected
            .iter()
            .zip(self.mac.iter())
            .fold(0u8, |difference, (left, right)| difference | (left ^ right))
            == 0
    }
}

/// Rejects replayed requests for one token. Only consulted after the MAC has
/// been verified, so unauthenticated traffic cannot fill it.
#[derive(Default)]
pub(super) struct ReplayGuard {
    newest: Option<u64>,
    floor: Option<u64>,
    seen: VecDeque<(u64, String)>,
}

impl ReplayGuard {
    pub(super) fn accept(&mut self, credentials: &Credentials) -> bool {
        let time = credentials.time;
        if self
            .newest
            .is_some_and(|newest| time.saturating_add(REPLAY_WINDOW_MS) < newest)
            || self.floor.is_some_and(|floor| time <= floor)
            || self
                .seen
                .iter()
                .any(|(seen, nonce)| *seen == time && *nonce == credentials.nonce)
        {
            return false;
        }
        let newest = self.newest.map_or(time, |newest| newest.max(time));
        self.newest = Some(newest);
        self.seen.push_back((time, credentials.nonce.clone()));
        self.seen
            .retain(|(seen, _)| seen.saturating_add(REPLAY_WINDOW_MS) >= newest);
        while self.seen.len() > MAX_REMEMBERED {
            if let Some((evicted, _)) = self.seen.pop_front() {
                self.floor = Some(self.floor.map_or(evicted, |floor| floor.max(evicted)));
            }
        }
        true
    }
}

/// Build a header the way the Android client does. Test-only: the desktop
/// never authenticates to itself.
#[cfg(test)]
pub(super) fn test_authorization(token: &str, method: &str, path: &str, client: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CLOCK: AtomicU64 = AtomicU64::new(1_000_000);
    let time = CLOCK.fetch_add(1, Ordering::Relaxed);
    let nonce = super::pairing::random_id().unwrap();
    let mac: String = signature(token, method, path, client, time, &nonce)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{SCHEME} t={time}, n={nonce}, m={mac}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials(token: &str, time: u64, nonce: &str) -> Credentials {
        let mac: String = signature(token, "GET", "/v2/video", "c", time, nonce)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Credentials::parse(&format!("{SCHEME} t={time}, n={nonce}, m={mac}")).unwrap()
    }

    #[test]
    fn hmac_matches_rfc_4231_and_the_android_vector() {
        // RFC 4231 test case 2.
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        let hex: String = mac.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Shared with android/tests/AuthTest.java.
        let hex: String = signature(
            "0123456789abcdef0123456789abcdef",
            "GET",
            "/v2/video",
            "00000000000000000000000000000001",
            42,
            "ffeeddccbbaa99887766554433221100",
        )
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
        assert_eq!(hex, ANDROID_VECTOR);
    }

    const ANDROID_VECTOR: &str = "16031fcf008b7e0e89af754288a13fa5e9771529bf633b703b326b0fd70a9e18";

    #[test]
    fn credentials_are_bound_to_token_route_and_client() {
        let nonce = "00112233445566778899aabbccddeeff";
        let valid = credentials("secret", 5, nonce);
        assert!(valid.signed_by("secret", "GET", "/v2/video", "c"));
        assert!(!valid.signed_by("other", "GET", "/v2/video", "c"));
        assert!(!valid.signed_by("secret", "GET", "/v2/audio", "c"));
        assert!(!valid.signed_by("secret", "POST", "/v2/video", "c"));
        assert!(!valid.signed_by("secret", "GET", "/v2/video", "d"));
        for malformed in [
            "Bearer secret",
            "Pluriview-HMAC t=1, n=00, m=00",
            "Pluriview-HMAC t=-1, n=00112233445566778899aabbccddeeff, m=00",
            "Pluriview-HMAC t=1, t=2, n=00112233445566778899aabbccddeeff, m=00",
        ] {
            assert!(Credentials::parse(malformed).is_none(), "{malformed}");
        }
    }

    #[test]
    fn replayed_and_stale_requests_are_rejected() {
        let mut guard = ReplayGuard::default();
        let first = credentials("secret", 100_000, "00112233445566778899aabbccddeeff");
        assert!(guard.accept(&first));
        assert!(!guard.accept(&first));
        // Slightly out of order (parallel connections) is fine.
        assert!(guard.accept(&credentials(
            "secret",
            99_000,
            "10112233445566778899aabbccddeeff"
        )));
        assert!(guard.accept(&credentials(
            "secret",
            200_000,
            "20112233445566778899aabbccddeeff"
        )));
        // Far behind the newest accepted time is a replay.
        assert!(!guard.accept(&credentials(
            "secret",
            100_001,
            "30112233445566778899aabbccddeeff"
        )));
    }
}
