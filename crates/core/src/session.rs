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
    qr_secret_hash: [u8; 32],
    access_token_hash: Option<[u8; 32]>,
    created_at: Instant,
    created_wall: DateTime<Utc>,
    paired: bool,
}

#[derive(Debug)]
pub struct SessionManager {
    state: Mutex<SessionState>,
    qr_secret: String,
}

impl SessionManager {
    #[must_use]
    pub fn new() -> Self {
        let mut random = rand::rng();
        let pin = format!("{:06}", random.next_u32() % 1_000_000);
        let qr_secret = random_token(&mut random, 32);
        Self {
            state: Mutex::new(SessionState {
                session_id: Uuid::new_v4(),
                pin,
                qr_secret_hash: hash(&qr_secret),
                access_token_hash: None,
                created_at: Instant::now(),
                created_wall: Utc::now(),
                paired: false,
            }),
            qr_secret,
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
            expires_in_seconds: UNPAIRED_TTL.saturating_sub(state.created_at.elapsed()).as_secs(),
            mode,
            created_at: state.created_wall,
        }
    }

    #[must_use]
    pub fn pin(&self) -> String {
        self.state.lock().pin.clone()
    }

    #[must_use]
    pub fn qr_secret(&self) -> &str {
        &self.qr_secret
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
    #[error("the pairing window expired; restart NFiDB to create a new session")]
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
            .pair_with_qr_secret(qr_session.qr_secret())
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
}
