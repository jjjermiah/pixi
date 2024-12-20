use std::fmt::Display;

use url::Url;

/// A specification of a package from a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GitSpec {
    /// The git url of the package
    pub git: Url,

    /// The git revision of the package
    #[serde(skip_serializing_if = "Option::is_none", flatten)]
    pub rev: Option<GitReference>,
}

/// A reference to a specific commit in a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitReference {
    /// The HEAD commit of a branch.
    Branch(String),

    /// A specific tag.
    Tag(String),

    /// A specific commit.
    Rev(String),
}

impl Display for GitReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitReference::Branch(branch) => write!(f, "{}", branch),
            GitReference::Tag(tag) => write!(f, "{}", tag),
            GitReference::Rev(rev) => write!(f, "{}", rev),
        }
    }
}
