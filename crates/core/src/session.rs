use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;
use uuid::Uuid;

const UNPAIRED_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug)]
struct SessionState {
    session_id: Uuid,
    pin: String,
    qr_secret: String,
    qr_secret_hash: [u8; 32],
    access_token_hash: Option<[u8; 32]>,
    created_at: Instant,
    created_wall: DateTime<Utc>,
    paired: bool,
}

#[derive(Debug)]
pub struct SessionManager {
    state: Mutex<SessionState>,
}

impl SessionManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(new_session_state()),
        }
    }

    #[must_use]
    pub fn public(&self, host_name: String, mode: String) -> PublicSession {
        let state = self.state.lock();
        PublicSession {
            product: "NFiDB".to_owned(),
            host_name,
            session_id: state.session_id,
            paired: state.paired,
            expires_in_seconds: if state.paired {
                UNPAIRED_TTL.as_secs()
            } else {
                UNPAIRED_TTL.saturating_sub(state.created_at.elapsed()).as_secs()
            },
            mode,
            created_at: state.created_wall,
        }
    }

    #[must_use]
    pub fn pin(&self) -> String {
        self.state.lock().pin.clone()
    }

    #[must_use]
    pub fn session_id(&self) -> Uuid {
        self.state.lock().session_id
    }

    #[must_use]
    pub fn qr_secret(&self) -> String {
        self.state.lock().qr_secret.clone()
    }

    #[must_use]
    pub fn expires_in_seconds(&self) -> u64 {
        let state = self.state.lock();
        if state.paired {
            UNPAIRED_TTL.as_secs()
        } else {
            UNPAIRED_TTL.saturating_sub(state.created_at.elapsed()).as_secs()
        }
    }

    #[must_use]
    pub fn is_paired(&self) -> bool {
        self.state.lock().paired
    }

    pub fn rotate(&self) {
        *self.state.lock() = new_session_state();
    }

    pub fn rotate_if_expired(&self) -> bool {
        let mut state = self.state.lock();
        if !state.paired && state.created_at.elapsed() >= UNPAIRED_TTL {
            *state = new_session_state();
            true
        } else {
            false
        }
    }

    pub fn pair_with_pin(&self, pin: &str) -> Result<PairResult, SessionError> {
        let mut state = self.state.lock();
        ensure_not_expired(&state)?;
        if !constant_eq(state.pin.as_bytes(), pin.trim().as_bytes()) {
            return Err(SessionError::InvalidCredentials);
        }
        issue_access_token(&mut state, PairMethod::Pin)
    }

    pub fn pair_with_qr_secret(&self, secret: &str) -> Result<PairResult, SessionError> {
        let mut state = self.state.lock();
        ensure_not_expired(&state)?;
        if !constant_eq(&state.qr_secret_hash, &hash(secret)) {
            return Err(SessionError::InvalidCredentials);
        }
        issue_access_token(&mut state, PairMethod::Qr)
    }

    #[must_use]
    pub fn authorize(&self, token: &str) -> bool {
        let state = self.state.lock();
        state
            .access_token_hash
            .is_some_and(|expected| constant_eq(&expected, &hash(token)))
    }

    pub fn disconnect(&self) {
        let mut state = self.state.lock();
        state.access_token_hash = None;
        state.paired = false;
        // Keep the displayed PIN and QR secret stable across an ordinary
        // reconnect, but start a fresh unpaired grace period. Credentials only
        // change on an explicit reset or after that full grace period elapses.
        state.created_at = Instant::now();
        state.created_wall = Utc::now();
    }
}

fn new_session_state() -> SessionState {
    let mut random = rand::rng();
    let pin = format!("{:06}", random.next_u32() % 1_000_000);
    let qr_secret = random_token(&mut random, 32);
    SessionState {
        session_id: Uuid::new_v4(),
        pin,
        qr_secret_hash: hash(&qr_secret),
        qr_secret,
        access_token_hash: None,
        created_at: Instant::now(),
        created_wall: Utc::now(),
        paired: false,
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

fn issue_access_token(state: &mut SessionState, method: PairMethod) -> Result<PairResult, SessionError> {
    let token = random_token(&mut rand::rng(), 32);
    state.access_token_hash = Some(hash(&token));
    state.paired = true;
    Ok(PairResult {
        access_token: token,
        session_id: state.session_id,
        method,
    })
}

fn ensure_not_expired(state: &SessionState) -> Result<(), SessionError> {
    if !state.paired && state.created_at.elapsed() > UNPAIRED_TTL {
        Err(SessionError::Expired)
    } else {
        Ok(())
    }
}

fn random_token(random: &mut impl RngCore, bytes: usize) -> String {
    let mut data = vec![0_u8; bytes];
    random.fill_bytes(&mut data);
    URL_SAFE_NO_PAD.encode(data)
}

fn hash(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn constant_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PairMethod {
    Pin,
    Qr,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairResult {
    pub access_token: String,
    pub session_id: Uuid,
    pub method: PairMethod,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicSession {
    pub product: String,
    pub host_name: String,
    pub session_id: Uuid,
    pub paired: bool,
    pub expires_in_seconds: u64,
    pub mode: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionError {
    #[error("the pairing code expired; reset the PIN and QR code on Windows")]
    Expired,
    #[error("invalid pairing credentials")]
    InvalidCredentials,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_and_qr_pairing_issue_valid_tokens() {
        let pin_session = SessionManager::new();
        let result = pin_session.pair_with_pin(&pin_session.pin()).expect("valid pin");
        assert!(pin_session.authorize(&result.access_token));
        assert!(!pin_session.authorize("wrong"));

        let qr_session = SessionManager::new();
        let result = qr_session
            .pair_with_qr_secret(&qr_session.qr_secret())
            .expect("valid QR secret");
        assert!(qr_session.authorize(&result.access_token));
    }

    #[test]
    fn wrong_pin_and_disconnect_are_rejected() {
        let session = SessionManager::new();
        assert!(matches!(
            session.pair_with_pin("000000"),
            Err(SessionError::InvalidCredentials)
        ));
        let result = session.pair_with_pin(&session.pin()).expect("valid pin");
        session.disconnect();
        assert!(!session.authorize(&result.access_token));
    }

    #[test]
    fn rotating_credentials_invalidates_pin_qr_and_access_token() {
        let session = SessionManager::new();
        let old_pin = session.pin();
        let old_qr = session.qr_secret();
        let result = session.pair_with_pin(&old_pin).expect("valid pin");
        session.rotate();
        assert!(!session.authorize(&result.access_token));
        assert!(matches!(
            session.pair_with_pin(&old_pin),
            Err(SessionError::InvalidCredentials)
        ));
        assert!(matches!(
            session.pair_with_qr_secret(&old_qr),
            Err(SessionError::InvalidCredentials)
        ));
        assert!(session.expires_in_seconds() > 590);
    }

    #[test]
    fn paired_credentials_do_not_appear_expired_and_survive_a_disconnect_grace_period() {
        let session = SessionManager::new();
        let pin = session.pin();
        let result = session.pair_with_pin(&pin).expect("valid pin");
        {
            let mut state = session.state.lock();
            state.created_at = Instant::now() - UNPAIRED_TTL - Duration::from_secs(1);
        }
        let public = session.public("test-host".to_owned(), "pen-display".to_owned());
        assert!(public.paired);
        assert_eq!(public.expires_in_seconds, UNPAIRED_TTL.as_secs());
        assert!(session.authorize(&result.access_token));

        session.disconnect();
        assert!(session.expires_in_seconds() > 590);
        assert!(session.pair_with_pin(&pin).is_ok());
    }
}
