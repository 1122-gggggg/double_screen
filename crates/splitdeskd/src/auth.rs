#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFailure {
    Missing,
    Mismatch,
}

impl AuthFailure {
    pub fn code(self) -> &'static str {
        "Auth"
    }

    pub fn message(self) -> &'static str {
        match self {
            AuthFailure::Missing => "token required",
            AuthFailure::Mismatch => "invalid token",
        }
    }
}

pub fn authorize(provided: &Option<String>, expected: &str) -> Result<(), AuthFailure> {
    match provided.as_deref().map(str::trim) {
        None | Some("") => Err(AuthFailure::Missing),
        Some(got) => {
            if token_eq(got, expected) {
                Ok(())
            } else {
                Err(AuthFailure::Mismatch)
            }
        }
    }
}

fn token_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}
