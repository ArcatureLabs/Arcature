//! [`Head`]: the per-page document metadata a server-rendered page carries --
//! `<title>`, the meta description, the canonical URL, and the Open Graph and
//! Twitter card fields that decide what a link preview looks like.
//!
//! # Why this exists at all
//!
//! An Inertia page's title and meta tags are normally set by the client, which
//! means they exist only after JavaScript has run. Google runs JavaScript.
//! Facebook, Zalo, Slack, Discord, LinkedIn, Telegram and X do not -- their
//! scrapers read the bytes the server sent and stop. A public Inertia page
//! therefore has no usable link preview no matter how good its client-side
//! `<Head>` component is.
//!
//! `Head` is the server's half: metadata the handler decides, carried to the
//! root document on the [`ScriptBody`](super::ScriptBody), and written into
//! the HTML before a byte of JavaScript is parsed.
//!
//! This is **not** server-side rendering. Nothing here executes application
//! JavaScript; there is no JS runtime in the request path. It is a handful of
//! `<meta>` tags, which is all a scraper reads.
//!
//! # Escaping is not the caller's job
//!
//! Every value is HTML-escaped **as it is stored**, in the setter. A page
//! title is routinely a row from the database -- an article headline, a
//! product name, a user's display name -- and interpolating one of those into
//! `<title>` or into a `content="..."` attribute unescaped is a textbook
//! stored-XSS hole. Escaping at render time would work too, right up until
//! the one root document that forgets. Escaping at construction time makes
//! forgetting impossible: by the time a value is reachable through an
//! accessor it is already safe to interpolate.
//!
//! The consequence to know about: accessors return **escaped** text. Feeding
//! an accessor's output back into a setter double-escapes it.

/// Per-page document metadata written into the server-rendered HTML.
///
/// Built with the `with_*` setters, which escape as they store, and read back
/// with the matching accessors, which return that escaped text. [`to_html`]
/// renders the whole set as `<title>`, `<meta>` and `<link>` elements.
///
/// ```
/// use arcature::inertia::Head;
///
/// let head = Head::new()
///     .with_title("Ada Lovelace")
///     .with_description("Notes on the Analytical Engine.")
///     .with_canonical("https://example.com/people/ada")
///     .with_og_image("https://example.com/og/ada.png");
///
/// let html = head.to_html();
/// assert!(html.contains("<title>Ada Lovelace</title>"));
/// assert!(html.contains(r#"<meta property="og:title" content="Ada Lovelace" />"#));
/// assert!(html.contains(r#"<meta name="twitter:card" content="summary_large_image" />"#));
///
/// // A title out of the database cannot break out of the element -- not in
/// // `<title>`, and not in the `content="..."` attribute it also feeds.
/// let hostile = Head::new().with_title("<script>alert(1)</script>").to_html();
/// assert!(!hostile.contains("<script>"));
/// assert!(hostile.contains("<title>&lt;script&gt;alert(1)&lt;/script&gt;</title>"));
/// ```
///
/// [`to_html`]: Head::to_html
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Head {
    title: Option<String>,
    description: Option<String>,
    canonical: Option<String>,
    og_title: Option<String>,
    og_description: Option<String>,
    og_type: Option<String>,
    og_url: Option<String>,
    og_image: Option<String>,
    og_image_alt: Option<String>,
    og_site_name: Option<String>,
    og_locale: Option<String>,
    twitter_card: Option<String>,
    twitter_site: Option<String>,
    twitter_creator: Option<String>,
    twitter_title: Option<String>,
    twitter_description: Option<String>,
    twitter_image: Option<String>,
}

/// Generate the accessor / setter pair for one metadata field.
///
/// Written as a macro because seventeen hand-copied pairs is seventeen
/// chances to store into the wrong field or to forget the [`escape`] call --
/// and forgetting the escape is the whole bug class this type exists to
/// close.
macro_rules! head_field {
    ($field:ident, $with:ident, $what:expr) => {
        #[doc = concat!("The ", $what, ", HTML-escaped, if it is set.")]
        ///
        /// The value is already safe to interpolate into element text or into
        /// a double-quoted attribute; it is *not* the original input.
        #[must_use]
        pub fn $field(&self) -> Option<&str> {
            self.$field.as_deref()
        }

        #[doc = concat!("Set the ", $what, ".")]
        ///
        /// The value is HTML-escaped here, at the point it is stored, so no
        /// renderer downstream can forget to do it.
        #[must_use]
        pub fn $with(mut self, value: impl AsRef<str>) -> Self {
            self.$field = Some(escape(value.as_ref()));
            self
        }
    };
}

impl Head {
    /// An empty head. Every field is unset and [`to_html`](Self::to_html)
    /// renders the empty string.
    #[must_use]
    pub fn new() -> Head {
        Head::default()
    }

    /// A head whose title is derived from an Inertia component identity.
    ///
    /// The last path segment is un-camel-cased and un-kebab-cased into words:
    /// `"users/index"` becomes `"Index"`, `"admin/user-settings"` becomes
    /// `"User Settings"`, `"NewLink"` becomes `"New Link"`. An identity with
    /// no word characters at all yields an empty head rather than an empty
    /// `<title>`.
    ///
    /// This is a *last resort*, not a recommendation -- a handler that knows
    /// the record it is rendering should say so with
    /// [`with_title`](Self::with_title). It exists because the alternative
    /// default is every page in the application sharing one title, which is
    /// among the worst things a site can do to its own search results.
    ///
    /// ```
    /// use arcature::inertia::Head;
    ///
    /// assert_eq!(Head::for_component("users/index").title(), Some("Index"));
    /// assert_eq!(Head::for_component("admin/user-settings").title(), Some("User Settings"));
    /// assert_eq!(Head::for_component("NewLink").title(), Some("New Link"));
    /// assert_eq!(Head::for_component("/").title(), None);
    /// ```
    #[must_use]
    pub fn for_component(component: &str) -> Head {
        match humanize_component(component) {
            Some(title) => Head::new().with_title(title),
            None => Head::new(),
        }
    }

    head_field!(title, with_title, "document title");
    head_field!(description, with_description, "meta description");
    head_field!(canonical, with_canonical, "canonical URL");
    head_field!(og_title, with_og_title, "`og:title`");
    head_field!(og_description, with_og_description, "`og:description`");
    head_field!(og_type, with_og_type, "`og:type`");
    head_field!(og_url, with_og_url, "`og:url`");
    head_field!(og_image, with_og_image, "`og:image`");
    head_field!(og_image_alt, with_og_image_alt, "`og:image:alt`");
    head_field!(og_site_name, with_og_site_name, "`og:site_name`");
    head_field!(og_locale, with_og_locale, "`og:locale`");
    head_field!(twitter_card, with_twitter_card, "`twitter:card` type");
    head_field!(twitter_site, with_twitter_site, "`twitter:site` handle");
    head_field!(
        twitter_creator,
        with_twitter_creator,
        "`twitter:creator` handle"
    );
    head_field!(twitter_title, with_twitter_title, "`twitter:title`");
    head_field!(
        twitter_description,
        with_twitter_description,
        "`twitter:description`"
    );
    head_field!(twitter_image, with_twitter_image, "`twitter:image`");

    /// Whether no field is set, in which case
    /// [`to_html`](Self::to_html) renders nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Head::default()
    }

    /// Render the metadata as HTML elements, one per line, separated by
    /// `"\n  "` so the block drops into an indented `<head>` unchanged. There
    /// is no leading indent and no trailing newline.
    ///
    /// Three fields fall back rather than being emitted twice by hand, because
    /// a preview that silently omits the title is the failure this type exists
    /// to prevent:
    ///
    /// - `og:title` falls back to the document title,
    /// - `og:description` falls back to the meta description,
    /// - `og:url` falls back to the canonical URL.
    ///
    /// `twitter:card` is emitted whenever there is anything to preview and no
    /// explicit card type was set: `summary_large_image` when an image is
    /// present, `summary` otherwise. X renders no card at all without that
    /// tag, so defaulting it is the difference between a preview and a bare
    /// link -- and unlike the other fields it has only two sensible values.
    ///
    /// ```
    /// use arcature::inertia::Head;
    ///
    /// assert_eq!(Head::new().to_html(), "");
    ///
    /// // og:title and og:url are not repeated by hand.
    /// let head = Head::new()
    ///     .with_title("Release notes")
    ///     .with_canonical("https://example.com/notes");
    /// let html = head.to_html();
    /// assert!(html.contains(r#"<meta property="og:title" content="Release notes" />"#));
    /// assert!(html.contains(r#"<meta property="og:url" content="https://example.com/notes" />"#));
    /// assert!(html.contains(r#"<meta name="twitter:card" content="summary" />"#));
    /// ```
    #[must_use]
    pub fn to_html(&self) -> String {
        let mut out = String::new();

        if let Some(title) = &self.title {
            push_element(&mut out, &format!("<title>{title}</title>"));
        }
        push_meta(&mut out, "name", "description", self.description.as_deref());
        if let Some(canonical) = &self.canonical {
            push_element(
                &mut out,
                &format!("<link rel=\"canonical\" href=\"{canonical}\" />"),
            );
        }

        let og_title = self.og_title.as_deref().or(self.title.as_deref());
        let og_description = self
            .og_description
            .as_deref()
            .or(self.description.as_deref());
        let og_url = self.og_url.as_deref().or(self.canonical.as_deref());

        push_meta(&mut out, "property", "og:title", og_title);
        push_meta(&mut out, "property", "og:description", og_description);
        push_meta(&mut out, "property", "og:type", self.og_type.as_deref());
        push_meta(&mut out, "property", "og:url", og_url);
        push_meta(&mut out, "property", "og:image", self.og_image.as_deref());
        push_meta(
            &mut out,
            "property",
            "og:image:alt",
            self.og_image_alt.as_deref(),
        );
        push_meta(
            &mut out,
            "property",
            "og:site_name",
            self.og_site_name.as_deref(),
        );
        push_meta(&mut out, "property", "og:locale", self.og_locale.as_deref());

        push_meta(&mut out, "name", "twitter:card", self.resolved_card());
        push_meta(
            &mut out,
            "name",
            "twitter:site",
            self.twitter_site.as_deref(),
        );
        push_meta(
            &mut out,
            "name",
            "twitter:creator",
            self.twitter_creator.as_deref(),
        );
        push_meta(
            &mut out,
            "name",
            "twitter:title",
            self.twitter_title.as_deref(),
        );
        push_meta(
            &mut out,
            "name",
            "twitter:description",
            self.twitter_description.as_deref(),
        );
        push_meta(
            &mut out,
            "name",
            "twitter:image",
            self.twitter_image.as_deref(),
        );

        out
    }

    /// The `twitter:card` value to emit: the explicit one, or a default
    /// picked from whether there is an image, or nothing when there is no
    /// preview content at all.
    fn resolved_card(&self) -> Option<&str> {
        if let Some(card) = self.twitter_card.as_deref() {
            return Some(card);
        }
        let has_image = self.twitter_image.is_some() || self.og_image.is_some();
        let has_content = has_image
            || self.title.is_some()
            || self.description.is_some()
            || self.og_title.is_some()
            || self.og_description.is_some()
            || self.twitter_title.is_some()
            || self.twitter_description.is_some();
        if !has_content {
            return None;
        }
        Some(if has_image {
            "summary_large_image"
        } else {
            "summary"
        })
    }
}

/// Append one `<meta>` element, or nothing when the value is unset.
///
/// `attribute` is `property` for Open Graph (RDFa, per the OGP spec) and
/// `name` for the plain HTML meta names Twitter uses. Getting that pair the
/// wrong way round is the single most common reason a link preview comes back
/// blank, which is why the choice is a parameter here rather than a guess
/// made at each call.
fn push_meta(out: &mut String, attribute: &str, key: &str, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    push_element(
        out,
        &format!("<meta {attribute}=\"{key}\" content=\"{value}\" />"),
    );
}

/// Append one element, separated from the previous one the way an indented
/// `<head>` block wants it.
fn push_element(out: &mut String, element: &str) {
    if !out.is_empty() {
        out.push_str("\n  ");
    }
    out.push_str(element);
}

/// Escape `raw` for both element text and double-quoted attribute values.
///
/// One function for both contexts on purpose. Two functions would mean each
/// call site picks, and a call site that picks wrong is an XSS hole that no
/// test notices; the superset is a few bytes larger and cannot be picked
/// wrong. `'` is escaped even though every attribute this module writes is
/// double-quoted, so a value that reaches a hand-written single-quoted
/// attribute in an application's own root document is still safe.
pub(crate) fn escape(raw: &str) -> String {
    if raw
        .bytes()
        .all(|b| b != b'&' && b != b'<' && b != b'>' && b != b'"' && b != b'\'')
    {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len() + raw.len() / 8);
    for character in raw.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            other => out.push(other),
        }
    }
    out
}

/// Turn a component identity into a human title, or `None` when there is no
/// word in it to turn.
fn humanize_component(component: &str) -> Option<String> {
    let segment = component.rsplit('/').find(|part| !part.is_empty())?;
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for character in segment.chars() {
        if character == '-' || character == '_' || character == '.' || character == ' ' {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            previous_lower = false;
            continue;
        }
        if character.is_uppercase() && previous_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        previous_lower = character.is_lowercase() || character.is_numeric();
        current.push(character);
    }
    if !current.is_empty() {
        words.push(current);
    }
    let title = words
        .iter()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() { None } else { Some(title) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_head_renders_nothing() {
        assert!(Head::new().is_empty());
        assert_eq!(Head::new().to_html(), "");
    }

    #[test]
    fn a_hostile_title_cannot_leave_the_title_element() {
        // The case this type exists for: a title straight out of a database
        // column somebody else filled in.
        let head = Head::new().with_title("<script>alert(1)</script>");
        let html = head.to_html();
        assert!(!html.contains("<script>"), "unescaped: {html}");
        assert!(html.contains("<title>&lt;script&gt;alert(1)&lt;/script&gt;</title>"));
    }

    #[test]
    fn a_hostile_description_cannot_leave_the_content_attribute() {
        let head = Head::new().with_description(r#"" onload="alert(1)"#);
        let html = head.to_html();
        assert!(!html.contains(r#"onload="alert"#), "unescaped: {html}");
        assert!(html.contains(r#"content="&quot; onload=&quot;alert(1)""#));
    }

    #[test]
    fn a_single_quote_is_escaped_too() {
        // Not needed by anything this module writes -- needed by the
        // hand-written root document that uses single-quoted attributes.
        let head = Head::new().with_og_site_name("Ada's");
        assert_eq!(head.og_site_name(), Some("Ada&#x27;s"));
    }

    #[test]
    fn an_ampersand_is_escaped_once_and_only_once() {
        let head = Head::new().with_title("Tea & Sympathy");
        assert_eq!(head.title(), Some("Tea &amp; Sympathy"));
    }

    #[test]
    fn a_plain_value_is_stored_unchanged() {
        let head = Head::new().with_title("Release notes");
        assert_eq!(head.title(), Some("Release notes"));
    }

    #[test]
    fn open_graph_falls_back_to_the_plain_fields() {
        let head = Head::new()
            .with_title("Release notes")
            .with_description("What changed.")
            .with_canonical("https://example.com/notes");
        let html = head.to_html();
        assert!(html.contains(r#"<meta property="og:title" content="Release notes" />"#));
        assert!(html.contains(r#"<meta property="og:description" content="What changed." />"#));
        assert!(html.contains(r#"<meta property="og:url" content="https://example.com/notes" />"#));
    }

    #[test]
    fn an_explicit_open_graph_value_wins_over_the_fallback() {
        let head = Head::new()
            .with_title("Release notes")
            .with_og_title("Arcature 0.1.1");
        let html = head.to_html();
        assert!(html.contains(r#"<meta property="og:title" content="Arcature 0.1.1" />"#));
        assert!(!html.contains(r#"content="Release notes""#));
    }

    #[test]
    fn the_twitter_card_defaults_to_the_large_image_when_there_is_one() {
        let head = Head::new()
            .with_title("Release notes")
            .with_og_image("https://example.com/og.png");
        assert!(
            head.to_html()
                .contains(r#"<meta name="twitter:card" content="summary_large_image" />"#)
        );
    }

    #[test]
    fn the_twitter_card_is_absent_when_there_is_nothing_to_preview() {
        let head = Head::new().with_canonical("https://example.com/notes");
        assert!(!head.to_html().contains("twitter:card"));
    }

    #[test]
    fn an_explicit_twitter_card_is_never_overridden() {
        let head = Head::new()
            .with_title("Release notes")
            .with_og_image("https://example.com/og.png")
            .with_twitter_card("summary");
        assert!(
            head.to_html()
                .contains(r#"<meta name="twitter:card" content="summary" />"#)
        );
    }

    #[test]
    fn open_graph_uses_property_and_twitter_uses_name() {
        // Swapping these is the classic reason a preview comes back blank.
        let head = Head::new()
            .with_og_type("article")
            .with_twitter_site("@arcature");
        let html = head.to_html();
        assert!(html.contains(r#"<meta property="og:type" content="article" />"#));
        assert!(html.contains(r#"<meta name="twitter:site" content="@arcature" />"#));
    }

    #[test]
    fn elements_are_separated_for_an_indented_head_block() {
        let head = Head::new().with_title("A").with_og_type("article");
        assert_eq!(
            head.to_html(),
            "<title>A</title>\n  \
             <meta property=\"og:title\" content=\"A\" />\n  \
             <meta property=\"og:type\" content=\"article\" />\n  \
             <meta name=\"twitter:card\" content=\"summary\" />"
        );
    }

    #[test]
    fn a_component_identity_becomes_a_human_title() {
        assert_eq!(Head::for_component("users/index").title(), Some("Index"));
        assert_eq!(
            Head::for_component("admin/user-settings").title(),
            Some("User Settings")
        );
        assert_eq!(Head::for_component("NewLink").title(), Some("New Link"));
        assert_eq!(
            Head::for_component("Posts/show_draft").title(),
            Some("Show Draft")
        );
    }

    #[test]
    fn a_component_identity_with_no_words_yields_no_title() {
        assert!(Head::for_component("/").is_empty());
        assert!(Head::for_component("").is_empty());
    }

    #[test]
    fn a_component_identity_is_escaped_like_everything_else() {
        assert_eq!(
            Head::for_component("<img src=x>").title(),
            Some("&lt;img Src=x&gt;")
        );
    }
}
