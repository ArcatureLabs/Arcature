//! [`ContractError`]: a registration failure in the typed page-contract
//! registry.

/// A registration failure in the typed page-contract registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractError {
    /// The same page identity was registered more than once.
    DuplicatePage(String),
    /// Two identities differ only by ASCII case. Rejected so a
    /// case-sensitive production frontend cannot disagree with a
    /// case-insensitive local filesystem.
    CaseConflict {
        /// The identity already in the registry.
        existing: String,
        /// The identity that was rejected.
        attempted: String,
    },
}

impl std::fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicatePage(page) => {
                write!(formatter, "duplicate Inertia page contract `{page}`")
            }
            Self::CaseConflict {
                existing,
                attempted,
            } => write!(
                formatter,
                "Inertia page contract `{attempted}` conflicts by case with `{existing}`"
            ),
        }
    }
}

impl std::error::Error for ContractError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_names_the_offending_page() {
        let error = ContractError::DuplicatePage("Home".into());
        assert_eq!(
            error.to_string(),
            "duplicate Inertia page contract `Home`"
        );
    }

    #[test]
    fn case_conflict_names_both_identities() {
        let error = ContractError::CaseConflict {
            existing: "Home".into(),
            attempted: "home".into(),
        };
        assert_eq!(
            error.to_string(),
            "Inertia page contract `home` conflicts by case with `Home`"
        );
    }
}
