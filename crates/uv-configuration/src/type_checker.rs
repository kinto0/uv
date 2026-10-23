use std::fmt::{self, Display, Formatter};

/// The type checker that `uv check` runs.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum TypeChecker {
    /// Use ty.
    #[default]
    Ty,
    /// Use Pyrefly.
    Pyrefly,
}

impl TypeChecker {
    /// The name of the type checker's package and executable.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ty => "ty",
            Self::Pyrefly => "pyrefly",
        }
    }
}

impl Display for TypeChecker {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
