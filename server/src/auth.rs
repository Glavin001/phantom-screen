use anyhow::{Context, Result};
use jsonwebtoken::{decode, DecodingKey, TokenData, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user identifier)
    pub sub: String,
    /// Expiration time (Unix timestamp)
    pub exp: usize,
    /// Issued at (Unix timestamp)
    #[serde(default)]
    pub iat: usize,
}

/// Validate a JWT token against the configured secret.
/// Returns the decoded claims on success.
pub fn validate_token(token: &str, secret: &str) -> Result<TokenData<Claims>> {
    let key = DecodingKey::from_secret(secret.as_bytes());
    let mut validation = Validation::default();
    // We only require sub and exp
    validation.required_spec_claims = ["exp", "sub"].iter().map(|s| s.to_string()).collect();
    validation.validate_exp = true;
    validation.validate_aud = false;

    decode::<Claims>(token, &key, &validation).context("Invalid JWT token")
}

/// Extract a token from a WebTransport session path.
/// Expected format: /session?token=<jwt> or /?token=<jwt>
pub fn extract_token_from_path(path: &str) -> Option<&str> {
    // Parse query string from path
    let query = path.split('?').nth(1)?;
    for param in query.split('&') {
        if let Some(value) = param.strip_prefix("token=") {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now_secs() -> usize {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize
    }

    #[test]
    fn test_validate_valid_token() {
        let secret = "test-secret-key";
        let claims = Claims {
            sub: "user123".to_string(),
            exp: now_secs() + 3600, // 1 hour from now
            iat: now_secs(),
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();

        let result = validate_token(&token, secret);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().claims.sub, "user123");
    }

    #[test]
    fn test_validate_expired_token() {
        let secret = "test-secret-key";
        let claims = Claims {
            sub: "user123".to_string(),
            exp: now_secs() - 100, // expired
            iat: now_secs() - 200,
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();

        let result = validate_token(&token, secret);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_wrong_secret() {
        let claims = Claims {
            sub: "user123".to_string(),
            exp: now_secs() + 3600,
            iat: now_secs(),
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(b"correct-secret"),
        )
        .unwrap();

        let result = validate_token(&token, "wrong-secret");
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_token_from_path() {
        assert_eq!(
            extract_token_from_path("/?token=abc123"),
            Some("abc123")
        );
        assert_eq!(
            extract_token_from_path("/session?token=xyz&foo=bar"),
            Some("xyz")
        );
        assert_eq!(extract_token_from_path("/session"), None);
        assert_eq!(extract_token_from_path("/?token="), None);
        assert_eq!(extract_token_from_path("/?foo=bar"), None);
    }
}
