//! The view subsystem's error type.
//!
//! One rule governs everything here: the detail of a render failure is for the
//! operator's log, never for the client's screen. An askama error message
//! names the template and can quote the value that would not format, and a
//! template path is a fragment of the source tree. Neither belongs in a
//! response body, so the conversion into [`crate::Error`] deliberately throws
//! the detail away -- after recording it.

use std::fmt;

/// A view could not be rendered.
///
/// The only way to build one is to render a [`View`](crate::view::View);
/// askama has already parsed and type-checked the template at build time, so
/// there is no parse error and no unknown-variable error to report at
/// runtime. What remains is a formatting failure: a value whose `Display`
/// impl returned `Err`, or a writer that refused the bytes.
#[derive(Debug)]
#[non_exhaustive]
pub enum ViewError {
    /// The template's writer or one of its values failed while formatting.
    Render {
        /// The underlying askama failure.
        source: askama::Error,
    },
}

impl From<askama::Error> for ViewError {
    fn from(source: askama::Error) -> Self {
        Self::Render { source }
    }
}

impl fmt::Display for ViewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Render { source } => write!(formatter, "view rendering failed: {source}"),
        }
    }
}

impl std::error::Error for ViewError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Render { source } => Some(source),
        }
    }
}

/// Fold a view failure into the framework error vocabulary as a plain
/// internal error.
///
/// The message is a constant. That is the point: `Error`'s own
/// [`IntoResponse`](axum::response::IntoResponse) writes its `Display` text
/// into the `detail` field of the problem document outside production, so
/// anything put in here is one `APP_ENV` away from being on the wire. A
/// template's text, the name of the field that would not format and the path
/// the template was read from are all things an attacker would like to know
/// and a user cannot act on.
///
/// This is also the only place the detail can be recorded, because it is
/// where it is dropped -- hence the `tracing` call rather than a silent
/// discard. `tracing` arrives with the `observe` feature and `views` does not
/// imply it; without `observe` there is nowhere to put the message and it
/// goes.
impl From<ViewError> for crate::Error {
    fn from(error: ViewError) -> Self {
        report(&error);
        crate::Error::Other("view rendering failed".into())
    }
}

/// Record a render failure for the operator.
fn report(error: &ViewError) {
    #[cfg(feature = "observe")]
    tracing::error!(%error, "a view failed to render; the client gets a generic 500");
    #[cfg(not(feature = "observe"))]
    let _ = error;
}

#[cfg(test)]
mod tests {
    use crate::view::test_support::Unformattable;
    use crate::view::view;

    #[test]
    fn the_framework_error_carries_no_template_detail() {
        let failure = view(Unformattable::default()).render().unwrap_err();
        let detail = failure.to_string();
        // The `ViewError` itself is allowed to be specific: it is what goes
        // to the log.
        assert!(detail.starts_with("view rendering failed"));

        let framework: crate::Error = failure.into();
        // What crosses into the response vocabulary is not.
        assert_eq!(framework.status(), 500);
        assert_eq!(framework.code(), "internal_error");
        assert_eq!(
            framework.to_string(),
            "internal error: view rendering failed"
        );
    }
}
