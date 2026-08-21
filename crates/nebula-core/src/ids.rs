use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn generate() -> Self {
                Self(ulid::Ulid::new().to_string())
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
id_newtype!(NoteId);

/// Id of the built-in workspace every install starts with (and the home of
/// projects that predate workspaces). A fixed literal, not a ULID, so the
/// store migration can reference it.
pub const DEFAULT_WORKSPACE_ID: &str = "default";

impl Default for WorkspaceId {
    fn default() -> Self {
        Self(DEFAULT_WORKSPACE_ID.into())
    }
}
