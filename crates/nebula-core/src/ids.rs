use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn generate() -> Self {
                Self(ulid::Ulid::generate().to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }
    };
}

id_newtype!(WorkspaceId);
id_newtype!(ProjectId);
id_newtype!(WorktreeId);
id_newtype!(AgentId);
id_newtype!(TerminalId);
id_newtype!(LinkId);

/// Id of the built-in workspace every install starts with (and the home of
/// projects that predate workspaces). A fixed literal, not a ULID, so the
/// store migration can reference it.
pub const DEFAULT_WORKSPACE_ID: &str = "default";

impl Default for WorkspaceId {
    fn default() -> Self {
        Self(DEFAULT_WORKSPACE_ID.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ids are stored as text in SQLite and sorted lexicographically, so the
    /// generated shape is a data format: 26 chars of Crockford base32, and
    /// later ids must sort after earlier ones.
    #[test]
    fn generated_ids_are_sortable_crockford_ulids() {
        let a = AgentId::generate();
        let b = AgentId::generate();
        for id in [&a, &b] {
            assert_eq!(id.as_str().len(), 26, "not a ULID: {id}");
            assert!(
                id.as_str().bytes().all(|c| c.is_ascii_digit()
                    || (c.is_ascii_uppercase()
                        && c != b'I'
                        && c != b'L'
                        && c != b'O'
                        && c != b'U')),
                "not Crockford base32: {id}"
            );
        }
        // Only the 10-char timestamp prefix is ordered; the 16-char random
        // suffix is not, so ids minted in the same millisecond may sort
        // either way. Text sort tracks creation order to the millisecond.
        assert!(
            a.as_str()[..10] <= b.as_str()[..10],
            "timestamps not monotonic: {a} then {b}"
        );
    }
}
