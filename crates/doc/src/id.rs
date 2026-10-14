//! Stable IDs (§3.2): a type prefix plus 10 Crockford base32 characters of
//! randomness, e.g. `n_8k2f9q4m1x`.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Crockford base32 alphabet, lowercase (no i, l, o, u).
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
/// Number of random characters after the prefix.
pub const ID_RANDOM_LEN: usize = 10;

/// What an ID names. Decides the prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IdKind {
    Node,
    Page,
    Style,
    Component,
}

impl IdKind {
    pub fn prefix(self) -> &'static str {
        match self {
            IdKind::Node => "n_",
            IdKind::Page => "p_",
            IdKind::Style => "s_",
            IdKind::Component => "c_",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    #[error("ID {0:?} has an unknown prefix (expected n_, p_, s_ or c_)")]
    Prefix(String),
    #[error("ID {0:?} must have exactly 10 Crockford base32 characters after the prefix")]
    Body(String),
    #[error("ID {id:?} is a {found:?} ID, expected a {expected:?} ID")]
    WrongKind {
        id: String,
        expected: IdKind,
        found: IdKind,
    },
}

/// A document ID. Stored as its string form; deserialization accepts any
/// string so a malformed file can still be opened, and `validate` reports
/// malformed IDs.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id(String);

pub type NodeId = Id;
pub type PageId = Id;
pub type StyleId = Id;
pub type ComponentId = Id;

impl Id {
    /// A fresh random ID of the given kind.
    pub fn new(kind: IdKind) -> Id {
        // The low 50 bits of bytes 0..8 (little-endian) are bytes 0..6 plus
        // the low 2 bits of byte 6; the v4 version nibble is byte 6's high
        // half, so all 50 bits are random.
        let bytes = *uuid::Uuid::new_v4().as_bytes();
        let bits = u64::from_le_bytes(bytes[..8].try_into().expect("8 bytes"));
        Id::from_entropy(kind, bits)
    }

    /// Build an ID from the low 50 bits of `bits`. Deterministic; used by
    /// tests and generators that need reproducible IDs.
    pub fn from_entropy(kind: IdKind, bits: u64) -> Id {
        let mut s = String::with_capacity(2 + ID_RANDOM_LEN);
        s.push_str(kind.prefix());
        for i in (0..ID_RANDOM_LEN).rev() {
            s.push(ALPHABET[((bits >> (i * 5)) & 31) as usize] as char);
        }
        Id(s)
    }

    /// Parse and validate an ID, returning it with its kind.
    pub fn parse(s: &str) -> Result<(Id, IdKind), IdError> {
        let kind = match s.get(..2) {
            Some("n_") => IdKind::Node,
            Some("p_") => IdKind::Page,
            Some("s_") => IdKind::Style,
            Some("c_") => IdKind::Component,
            _ => return Err(IdError::Prefix(s.to_string())),
        };
        let body = &s.as_bytes()[2..];
        if body.len() != ID_RANDOM_LEN || !body.iter().all(|b| ALPHABET.contains(b)) {
            return Err(IdError::Body(s.to_string()));
        }
        Ok((Id(s.to_string()), kind))
    }

    /// Check this ID is well formed and of the expected kind.
    pub fn check(&self, expected: IdKind) -> Result<(), IdError> {
        let (_, found) = Id::parse(&self.0)?;
        if found != expected {
            return Err(IdError::WrongKind {
                id: self.0.clone(),
                expected,
                found,
            });
        }
        Ok(())
    }

    /// Wrap a string without validation (e.g. instance-derived IDs
    /// `<instanceId>/<componentNodeId>`, §3.7).
    pub fn from_raw(s: impl Into<String>) -> Id {
        Id(s.into())
    }

    /// The derived ID of a component node shown inside an instance.
    pub fn derived(instance: &Id, component_node: &Id) -> Id {
        Id(format!("{}/{}", instance.0, component_node.0))
    }

    pub fn is_derived(&self) -> bool {
        self.0.contains('/')
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_ids_parse_with_their_kind() {
        for kind in [IdKind::Node, IdKind::Page, IdKind::Style, IdKind::Component] {
            let id = Id::new(kind);
            assert_eq!(id.as_str().len(), 12);
            assert!(id.as_str().starts_with(kind.prefix()));
            assert_eq!(Id::parse(id.as_str()).unwrap().1, kind);
            assert!(id.check(kind).is_ok());
        }
    }

    #[test]
    fn new_ids_differ() {
        assert_ne!(Id::new(IdKind::Node), Id::new(IdKind::Node));
    }

    #[test]
    fn from_entropy_uses_low_50_bits() {
        assert_eq!(Id::from_entropy(IdKind::Node, 0).as_str(), "n_0000000000");
        assert_eq!(Id::from_entropy(IdKind::Node, 31).as_str(), "n_000000000z");
        assert_eq!(
            Id::from_entropy(IdKind::Page, u64::MAX).as_str(),
            "p_zzzzzzzzzz"
        );
        assert_eq!(
            Id::from_entropy(IdKind::Node, 1 << 50),
            Id::from_entropy(IdKind::Node, 0)
        );
    }

    #[test]
    fn parse_rejects_bad_ids() {
        assert!(matches!(Id::parse("x_8k2f9q4m1x"), Err(IdError::Prefix(_))));
        assert!(matches!(Id::parse("n_8k2f9q4m1"), Err(IdError::Body(_))));
        assert!(matches!(Id::parse("n_8k2f9q4m1xx"), Err(IdError::Body(_))));
        // i, l, o and u are not Crockford base32 digits; nor are capitals.
        assert!(matches!(Id::parse("n_5t6y7u8i9o"), Err(IdError::Body(_))));
        assert!(matches!(Id::parse("n_8K2F9Q4M1X"), Err(IdError::Body(_))));
        assert!(matches!(Id::parse(""), Err(IdError::Prefix(_))));
        assert!(matches!(
            Id::from_raw("p_0q1w2e3r4t").check(IdKind::Node),
            Err(IdError::WrongKind { .. })
        ));
    }

    #[test]
    fn derived_ids() {
        let i = Id::from_raw("n_0000000001");
        let c = Id::from_raw("n_0000000002");
        let d = Id::derived(&i, &c);
        assert_eq!(d.as_str(), "n_0000000001/n_0000000002");
        assert!(d.is_derived());
        assert!(!i.is_derived());
    }
}
