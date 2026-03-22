//! Portage atom validation.
//!
//! Validates package atoms before they are forwarded to `emerge`.

/// Validate a portage package atom.
///
/// Accepted forms:
/// - `package` — e.g. `firefox` (unqualified, emerge resolves the category)
/// - `category/package` — e.g. `dev-libs/openssl`
/// - `=category/package-version` — e.g. `=dev-libs/openssl-3.1.4`
/// - `>=category/package-version` — with version operator
/// - `@set` — e.g. `@world`, `@system`
///
/// Rejects atoms containing shell metacharacters or invalid syntax.
pub fn validate_atom(atom: &str) -> Result<(), AtomValidationError> {
    if atom.is_empty() {
        return Err(AtomValidationError::Empty);
    }

    // Reject shell metacharacters.  Note: `>` and `<` are NOT rejected
    // because they are valid portage version operator prefixes.
    const SHELL_CHARS: &[char] = &[
        '`', '$', '\\', '"', '\'', ';', '&', '|', '(', ')', '{', '}', '\n', '\r', '\0',
    ];
    if atom.contains(SHELL_CHARS) {
        return Err(AtomValidationError::DangerousCharacters);
    }

    // Package sets: @world, @system, @preserved-rebuild, etc.
    if let Some(set_name) = atom.strip_prefix('@') {
        if set_name.is_empty() {
            return Err(AtomValidationError::InvalidFormat("empty set name".into()));
        }
        if !set_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(AtomValidationError::InvalidFormat(
                "invalid set name characters".into(),
            ));
        }
        return Ok(());
    }

    // Strip version operator prefix if present.
    let stripped = atom
        .strip_prefix(">=")
        .or_else(|| atom.strip_prefix("<="))
        .or_else(|| atom.strip_prefix("="))
        .or_else(|| atom.strip_prefix(">"))
        .or_else(|| atom.strip_prefix("<"))
        .or_else(|| atom.strip_prefix("~"))
        .unwrap_or(atom);

    // Qualified atom: category/package.
    if let Some((category, package)) = stripped.split_once('/') {
        // Reject multiple slashes.
        if package.contains('/') {
            return Err(AtomValidationError::InvalidFormat(
                "too many '/' separators".into(),
            ));
        }

        if category.is_empty() {
            return Err(AtomValidationError::InvalidFormat("empty category".into()));
        }
        if !category
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(AtomValidationError::InvalidFormat(
                "invalid category characters".into(),
            ));
        }

        if package.is_empty() {
            return Err(AtomValidationError::InvalidFormat(
                "empty package name".into(),
            ));
        }
        return validate_package_name(package);
    }

    // Unqualified atom: bare package name (emerge resolves the category).
    // Version operators are not valid without a category.
    if stripped != atom {
        return Err(AtomValidationError::InvalidFormat(
            "version operators require a qualified category/package atom".into(),
        ));
    }

    validate_package_name(stripped)
}

/// Validate the package-name portion of an atom.
fn validate_package_name(package: &str) -> Result<(), AtomValidationError> {
    if package.is_empty() {
        return Err(AtomValidationError::InvalidFormat(
            "empty package name".into(),
        ));
    }
    // Package names may contain letters, digits, hyphens, underscores,
    // dots, and plus signs.  Version suffixes like `-3.1.4-r1` are also
    // valid.  Wildcards (`*`) are allowed in version globs.
    if !package
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '*'))
    {
        return Err(AtomValidationError::InvalidFormat(
            "invalid package name characters".into(),
        ));
    }

    Ok(())
}

/// Error returned when a portage atom is invalid.
#[derive(Debug, Clone, PartialEq)]
pub enum AtomValidationError {
    /// The atom string is empty.
    Empty,
    /// The atom contains shell metacharacters.
    DangerousCharacters,
    /// The atom format is invalid.
    InvalidFormat(String),
}

impl std::fmt::Display for AtomValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "atom is empty"),
            Self::DangerousCharacters => {
                write!(f, "atom contains dangerous shell metacharacters")
            }
            Self::InvalidFormat(msg) => write!(f, "invalid atom format: {msg}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_simple_atom() {
        assert!(validate_atom("dev-libs/openssl").is_ok());
        assert!(validate_atom("sys-kernel/gentoo-sources").is_ok());
        assert!(validate_atom("app-misc/screen").is_ok());
    }

    #[test]
    fn valid_versioned_atom() {
        assert!(validate_atom("=dev-libs/openssl-3.1.4").is_ok());
        assert!(validate_atom(">=dev-libs/openssl-3.0").is_ok());
        assert!(validate_atom("~dev-libs/openssl-3.1.4").is_ok());
        assert!(validate_atom("<dev-libs/openssl-4.0").is_ok());
    }

    #[test]
    fn valid_set() {
        assert!(validate_atom("@world").is_ok());
        assert!(validate_atom("@system").is_ok());
        assert!(validate_atom("@preserved-rebuild").is_ok());
    }

    #[test]
    fn valid_version_glob() {
        assert!(validate_atom("=dev-libs/openssl-3.*").is_ok());
    }

    #[test]
    fn reject_empty() {
        assert!(validate_atom("").is_err());
    }

    #[test]
    fn reject_shell_injection() {
        assert!(validate_atom("dev-libs/openssl; rm -rf /").is_err());
        assert!(validate_atom("dev-libs/openssl$(evil)").is_err());
        assert!(validate_atom("dev-libs/openssl`cmd`").is_err());
        assert!(validate_atom("dev-libs/openssl\"").is_err());
    }

    #[test]
    fn valid_unqualified_atom() {
        assert!(validate_atom("firefox").is_ok());
        assert!(validate_atom("openssl").is_ok());
        assert!(validate_atom("gentoo-sources").is_ok());
    }

    #[test]
    fn reject_versioned_unqualified_atom() {
        // Version operators require category/package.
        assert!(validate_atom("=firefox-128.0").is_err());
        assert!(validate_atom(">=openssl-3.0").is_err());
    }

    #[test]
    fn reject_empty_set() {
        assert!(validate_atom("@").is_err());
    }

    #[test]
    fn reject_empty_category() {
        assert!(validate_atom("/openssl").is_err());
    }

    #[test]
    fn reject_empty_package() {
        assert!(validate_atom("dev-libs/").is_err());
    }
}
