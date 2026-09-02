use argon2::{password_hash::phc::PasswordHash, Argon2, PasswordHasher, PasswordVerifier};
use axum::{
    extract::{FromRef, FromRequestParts},
    http::{header, request::Parts},
};
use rand_core_06::{OsRng, RngCore};
use std::sync::Arc;
use uuid::Uuid;

use crate::{error::ApiError, AppState};

#[allow(dead_code)]
pub struct AuthUser {
    pub id: Uuid,
    pub username: String,
    pub token: String,
}

impl<S> FromRequestParts<S> for AuthUser
where
    Arc<AppState>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let token_str = {
            let auth_header = parts
                .headers
                .get(header::AUTHORIZATION)
                .and_then(|h| h.to_str().ok())
                .ok_or_else(|| ApiError::Unauthorized("Missing Authorization header".into()))?;

            if !auth_header.starts_with("Bearer ") {
                return Err(ApiError::Unauthorized(
                    "Invalid Authorization header format".into(),
                ));
            }

            let t = auth_header.trim_start_matches("Bearer ").trim();
            if t.is_empty() {
                return Err(ApiError::Unauthorized("Empty token".into()));
            }
            t.to_string()
        };

        let app_state = Arc::<AppState>::from_ref(state);

        let user = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT u.id, u.username
             FROM users u
             INNER JOIN tokens t ON u.id = t.user_id
             WHERE t.token = $1 AND t.expires_at > now()",
        )
        .bind(&token_str)
        .fetch_optional(&app_state.db)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("Invalid or expired token".into()))?;

        Ok(AuthUser {
            id: user.0,
            username: user.1,
            token: token_str,
        })
    }
}

pub fn generate_opaque_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn hash_password(password: &str) -> Result<String, ApiError> {
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| ApiError::Internal(format!("Password hashing failed: {e}")))
}

pub fn verify_password(password: &str, hash_str: &str) -> bool {
    let parsed_hash = match PasswordHash::new(hash_str) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_hashing_and_verification() {
        let password = generate_opaque_token();
        let wrong_password = generate_opaque_token();
        let hash = hash_password(&password).expect("hashing should succeed");

        assert!(verify_password(&password, &hash));
        assert!(!verify_password(&wrong_password, &hash));
    }

    #[test]
    fn test_opaque_token_generation() {
        let t1 = generate_opaque_token();
        let t2 = generate_opaque_token();

        assert_eq!(t1.len(), 64);
        assert_ne!(t1, t2);
    }
}
