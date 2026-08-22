//! Turning an attacker-authored filename into something safe to store.
//!
//! Every byte of a `filename=` parameter in a `multipart/form-data` request is
//! written by the client. It is not a name, it is an argument to whatever
//! opens it next -- a path resolver, a shell, a web server's handler map, a
//! terminal. This module is the one place that argument is disarmed.
//!
//! # The shape of the problem
//!
//! Filenames attack in six different ways, and a filter that only knows about
//! one of them is a filter that has not been written yet:
//!
//! | Input | What it wants | What happens here |
//! |---|---|---|
//! | `../../etc/passwd` | escape the storage root | the directory part is discarded before anything else runs; the name is `passwd` |
//! | `a\0b.jpg` | truncate the name in a C API downstream | rejected: control characters are never sanitized away, they are fatal |
//! | `CON.txt` | open a Windows device instead of a file | rejected: reserved device names are refused, dots and trailing spaces included |
//! | `invoice\u{202E}gpj.exe` | render right-to-left so the extension reads `.jpg` | rejected: bidi and other invisible format controls are fatal |
//! | `shell.php.jpg` | be served by a mis-configured `AddHandler` | the second extension marker is replaced: `shell_php.jpg` |
//! | `Ảnh chụp màn hình.png` | *nothing* -- an ordinary Vietnamese filename | accepted, NFC-normalized, unchanged otherwise |
//!
//! That last row is the reason this module exists at all.
//! [`StoragePath::new`](crate::storage::StoragePath::new) validates and
//! rejects; it has no opinion about how to *repair* a name, so a filename with
//! a space or a diacritic in it fails a check that was never aimed at it. A
//! sanitizer that rejects real names teaches applications to bypass the
//! sanitizer.
//!
//! # Reject versus repair
//!
//! The split is deliberate. A character is **repaired** (replaced with `_`)
//! when a human plausibly typed it and it is only dangerous to a downstream
//! parser: `:`, `*`, `?`, `<`, `>`, `|`, `"`, and the extra `.` of a double
//! extension. A character is **rejected** when its presence is itself the
//! attack and no legitimate file manager produces it: NUL and the other C0/C1
//! controls, DEL, the bidi overrides, the zero-width joiners, the byte-order
//! mark, the blank-rendering separators and the Unicode tag block. Nobody
//! names a holiday photo with a right-to-left override in it.
//!
//! # This is defence in depth, not the defence
//!
//! The name this module returns is still metadata. An upload is meant to be
//! stored under a name derived from its own bytes, not from anything the
//! client wrote, so that even a bug in this file cannot put an
//! attacker-chosen string into a path. Keep both.

use std::collections::BTreeSet;
use std::fmt;

use unicode_normalization::UnicodeNormalization;

use crate::storage::error::FilenameError;

/// The longest input accepted before any work is done, in bytes.
///
/// A filename is a header parameter, not a payload. Anything past this is a
/// resource-exhaustion attempt against the normalizer, and rejecting it early
/// costs one comparison.
const MAX_INPUT_BYTES: usize = 4096;

/// The longest sanitized filename produced, in bytes.
///
/// 255 bytes is the per-component limit on ext4, XFS, APFS and NTFS alike. A
/// longer stem is truncated rather than refused, because the stored name is
/// content-addressed and the sanitized name is only metadata.
pub const MAX_FILENAME_BYTES: usize = 255;

/// The longest extension accepted, in bytes.
const MAX_EXTENSION_BYTES: usize = 16;

/// Windows device names. Opening any of these -- with any extension, and with
/// trailing dots or spaces -- talks to a device, not a file.
///
/// `COM0` and `LPT0` are included: they are not documented as reserved but
/// several Windows versions treat them as such, and no application needs a
/// file called `COM0`.
const RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM0", "COM1", "COM2", "COM3", "COM4",
    "COM5", "COM6", "COM7", "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9",
];

/// A validated file extension: lowercase ASCII alphanumerics, nothing else.
///
/// The restriction is not tidiness. An extension is the string a web server,
/// an operating system and a browser each use to decide what a file *is*, and
/// every one of them parses it slightly differently. Allowing only
/// `[0-9a-z]{1,16}` means there is no encoding, no separator and no
/// homoglyph left for those three parsers to disagree about.
///
/// # Example
///
/// ```
/// use arcature::storage::Extension;
///
/// // Case is normalized; the comparison downstream is then a plain `==`.
/// assert_eq!(Extension::parse("JPG").unwrap().as_str(), "jpg");
///
/// // Anything that is not an ASCII alphanumeric is refused outright.
/// assert!(Extension::parse("jpg.exe").is_err());
/// assert!(Extension::parse("jpg ").is_err());
/// assert!(Extension::parse("").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Extension(String);

impl Extension {
    /// Parse an extension, without the leading dot.
    ///
    /// # Errors
    ///
    /// Returns [`FilenameError::MissingExtension`] for an empty string and
    /// [`FilenameError::InvalidExtension`] for anything that is not one to
    /// [`MAX_EXTENSION_BYTES`] ASCII alphanumerics.
    pub fn parse(extension: &str) -> Result<Self, FilenameError> {
        if extension.is_empty() {
            return Err(FilenameError::MissingExtension);
        }
        if extension.len() > MAX_EXTENSION_BYTES {
            return Err(FilenameError::InvalidExtension);
        }
        if !extension.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(FilenameError::InvalidExtension);
        }
        Ok(Self(extension.to_ascii_lowercase()))
    }

    /// The extension as a `&str`, lowercase and without a leading dot.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Extension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for Extension {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// The set of extensions an application is willing to store.
///
/// A whitelist, never a blacklist. A blacklist of dangerous extensions is a
/// list of the ones somebody thought of, and the interesting ones are always
/// the other ones -- `.phtml`, `.php7`, `.cgi`, `.jsp`, `.svgz`, `.htaccess`.
/// An empty `AllowedExtensions` therefore stores nothing at all, which is the
/// correct behaviour for a misconfiguration.
///
/// # Example
///
/// ```
/// use arcature::storage::{AllowedExtensions, Extension};
///
/// let allowed = AllowedExtensions::images();
/// assert!(allowed.contains(&Extension::parse("png").unwrap()));
/// assert!(!allowed.contains(&Extension::parse("svg").unwrap()));
///
/// let custom = AllowedExtensions::new(["JPG", "pdf"]).unwrap();
/// assert!(custom.contains(&Extension::parse("jpg").unwrap()));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllowedExtensions {
    allowed: BTreeSet<Extension>,
}

impl AllowedExtensions {
    /// Build a whitelist from a list of extensions, without leading dots.
    ///
    /// # Errors
    ///
    /// Returns [`FilenameError::InvalidExtension`] if any entry is not a valid
    /// [`Extension`]. This is a programming error in the application, caught
    /// at the point the whitelist is written rather than on the first upload.
    pub fn new<I, S>(extensions: I) -> Result<Self, FilenameError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut allowed = BTreeSet::new();
        for extension in extensions {
            allowed.insert(Extension::parse(extension.as_ref())?);
        }
        Ok(Self { allowed })
    }

    /// The raster image formats: `jpg`, `jpeg`, `png`, `gif`, `webp`.
    ///
    /// `svg` is deliberately absent. An SVG is an XML document that may carry
    /// script, so serving one inline is a stored-XSS primitive; if an
    /// application needs SVG it should add it knowingly.
    #[must_use]
    pub fn images() -> Self {
        Self::new(["jpg", "jpeg", "png", "gif", "webp"])
            .expect("the built-in image extensions are valid")
    }

    /// The plain document formats: `pdf`, `txt`, `csv`.
    #[must_use]
    pub fn documents() -> Self {
        Self::new(["pdf", "txt", "csv"]).expect("the built-in document extensions are valid")
    }

    /// Whether this whitelist admits `extension`.
    #[must_use]
    pub fn contains(&self, extension: &Extension) -> bool {
        self.allowed.contains(extension)
    }

    /// The allowed extensions, in sorted order.
    pub fn iter(&self) -> impl Iterator<Item = &Extension> {
        self.allowed.iter()
    }

    /// How many extensions the whitelist admits.
    #[must_use]
    pub fn len(&self) -> usize {
        self.allowed.len()
    }

    /// Whether the whitelist is empty -- in which case nothing is storable.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.allowed.is_empty()
    }

    /// Add an extension to the whitelist.
    ///
    /// # Errors
    ///
    /// Returns [`FilenameError::InvalidExtension`] if `extension` is not a
    /// valid [`Extension`].
    pub fn with(mut self, extension: &str) -> Result<Self, FilenameError> {
        self.allowed.insert(Extension::parse(extension)?);
        Ok(self)
    }
}

impl<'a> IntoIterator for &'a AllowedExtensions {
    type Item = &'a Extension;
    type IntoIter = std::collections::btree_set::Iter<'a, Extension>;

    fn into_iter(self) -> Self::IntoIter {
        self.allowed.iter()
    }
}

/// A filename that has survived sanitization: a stem plus a whitelisted
/// [`Extension`], with no path, no controls and no device name in it.
///
/// This is **metadata**. It is what an application shows a user and what it
/// puts in a `Content-Disposition` header. It is never what the object is
/// stored under.
///
/// # Example
///
/// ```
/// use arcature::storage::{AllowedExtensions, SafeFilename};
///
/// let allowed = AllowedExtensions::images();
///
/// // A double extension loses its inner marker.
/// let name = SafeFilename::parse("shell.php.jpg", &allowed).unwrap();
/// assert_eq!(name.to_string(), "shell_php.jpg");
///
/// // A Windows path prefix is discarded, not escaped.
/// let name = SafeFilename::parse(r"C:\Users\me\holiday.PNG", &allowed).unwrap();
/// assert_eq!(name.to_string(), "holiday.png");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeFilename {
    stem: String,
    extension: Extension,
}

impl SafeFilename {
    /// Sanitize `filename` against `allowed`.
    ///
    /// # Errors
    ///
    /// Returns the [`FilenameError`] describing the first check that failed.
    /// The variants are deliberately coarse: the caller reports a fixed string
    /// to the client, never the offending input.
    pub fn parse(filename: &str, allowed: &AllowedExtensions) -> Result<Self, FilenameError> {
        if filename.is_empty() {
            return Err(FilenameError::Empty);
        }
        if filename.len() > MAX_INPUT_BYTES {
            return Err(FilenameError::TooLong);
        }

        // 1. Fatal characters first, over the *whole* input. Checking after
        //    the directory part is stripped would let `../\0/photo.jpg`
        //    launder a NUL past the check by hiding it in the discarded half.
        if filename.chars().any(is_fatal_char) {
            return Err(FilenameError::ControlChar);
        }

        // 2. Discard every directory component. Both separators, because the
        //    client chooses the platform, and a Unix server still has to cope
        //    with a Windows browser's `C:\Users\me\photo.jpg`.
        let base = filename
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .trim();
        if base.is_empty() {
            return Err(FilenameError::Empty);
        }
        if base == "." || base == ".." {
            return Err(FilenameError::Traversal);
        }

        // 3. Normalize to NFC so `ả` written as one code point and `ả` written
        //    as `a` plus two combining marks become the same string. Without
        //    this, two uploads of the same file can carry names that compare
        //    unequal while rendering identically.
        let base: String = base.nfc().collect();

        // 4. Windows opens a name with trailing dots and spaces by stripping
        //    them, so `CON.txt. ` is `CON`. Strip them here, before anything
        //    downstream can be fooled by the difference.
        let base = base.trim_end_matches(['.', ' ']).trim_start();
        if base.is_empty() {
            return Err(FilenameError::Empty);
        }

        // 5. A reserved device name is reserved whatever follows the first
        //    dot: `CON.txt` and `CON.foo.txt` both open `CON`.
        let leading = base
            .split('.')
            .next()
            .unwrap_or_default()
            .trim_end_matches([' ', '.']);
        if is_reserved_device_name(leading) {
            return Err(FilenameError::ReservedName);
        }

        // 6. Split at the *last* dot: that is the extension every consumer
        //    reads.
        let (stem_raw, extension_raw) = base
            .rsplit_once('.')
            .ok_or(FilenameError::MissingExtension)?;
        let extension = Extension::parse(extension_raw)?;
        if !allowed.contains(&extension) {
            return Err(FilenameError::ExtensionNotAllowed);
        }

        // 7. Repair the stem. Every remaining `.` is an inner extension
        //    marker, and the rest are characters some filesystem or shell
        //    treats as syntax.
        let stem: String = stem_raw
            .chars()
            .map(|ch| if is_repaired_char(ch) { '_' } else { ch })
            .collect();
        let stem = stem.trim().to_string();
        if stem.is_empty() {
            return Err(FilenameError::EmptyStem);
        }
        if is_reserved_device_name(&stem) {
            return Err(FilenameError::ReservedName);
        }

        // 8. Fit the whole name inside one filesystem component, cutting the
        //    stem on a character boundary. The extension is never truncated:
        //    a half extension is a different file type.
        let budget = MAX_FILENAME_BYTES.saturating_sub(extension.as_str().len() + 1);
        let stem = truncate_on_char_boundary(&stem, budget);
        if stem.is_empty() {
            return Err(FilenameError::EmptyStem);
        }

        Ok(Self {
            stem: stem.to_string(),
            extension,
        })
    }

    /// The sanitized stem, without the dot or the extension.
    #[must_use]
    pub fn stem(&self) -> &str {
        &self.stem
    }

    /// The whitelisted extension.
    #[must_use]
    pub fn extension(&self) -> &Extension {
        &self.extension
    }
}

impl fmt::Display for SafeFilename {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.stem, self.extension)
    }
}

/// Characters whose presence in a filename is itself the attack.
///
/// C0 and C1 controls, DEL, the bidirectional overrides and isolates, the
/// zero-width joiners and non-joiners, the word joiner, the byte-order mark,
/// the blank-rendering separators (soft hyphen, Mongolian vowel separator,
/// Hangul filler) and the whole Unicode tag block. Every one of them either
/// terminates a string early in a C API, makes a rendered filename lie about
/// its own extension, or is invisible in every font there is -- and two names
/// that render identically while comparing unequal is the whole of the
/// display-spoofing problem. None of them is produced by any file manager, so
/// repairing them would only hide the attempt.
///
/// The variation selectors (U+FE00-U+FE0F) are deliberately *not* here. They
/// occur in ordinary names beside an emoji, and refusing them would put this
/// function back in the business of rejecting filenames people really have.
fn is_fatal_char(ch: char) -> bool {
    matches!(ch,
        '\u{0}'..='\u{1F}'
        | '\u{7F}'..='\u{9F}'
        | '\u{00AD}'
        | '\u{061C}'
        | '\u{180E}'
        | '\u{200B}'..='\u{200F}'
        | '\u{2028}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206F}'
        | '\u{3164}'
        | '\u{FEFF}'
        | '\u{FFF9}'..='\u{FFFB}'
        | '\u{E0000}'..='\u{E007F}')
}

/// Characters replaced with `_` in the stem.
///
/// `.` is here because after the extension is split off, every remaining dot
/// is an inner extension marker -- the `shell.php.jpg` family. The rest are
/// the characters Windows refuses in a filename plus the two path separators,
/// which double as NTFS alternate-data-stream (`:`) and wildcard (`*`, `?`)
/// syntax.
fn is_repaired_char(ch: char) -> bool {
    matches!(
        ch,
        '.' | '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
    )
}

/// Whether `name` is a Windows device name, ignoring case.
fn is_reserved_device_name(name: &str) -> bool {
    RESERVED_DEVICE_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(name))
}

/// Truncate `text` to at most `budget` bytes, cutting on a character boundary.
fn truncate_on_char_boundary(text: &str, budget: usize) -> &str {
    if text.len() <= budget {
        return text;
    }
    let mut end = budget;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed() -> AllowedExtensions {
        AllowedExtensions::new(["jpg", "jpeg", "png", "txt", "pdf"]).unwrap()
    }

    fn parse(filename: &str) -> Result<SafeFilename, FilenameError> {
        SafeFilename::parse(filename, &allowed())
    }

    #[test]
    fn accepts_an_ordinary_name() {
        assert_eq!(parse("holiday.jpg").unwrap().to_string(), "holiday.jpg");
    }

    #[test]
    fn lowercases_the_extension() {
        assert_eq!(parse("HOLIDAY.JPG").unwrap().to_string(), "HOLIDAY.jpg");
    }

    #[test]
    fn accepts_a_vietnamese_name_with_spaces_and_diacritics() {
        let name = parse("Ảnh chụp màn hình.png").unwrap();
        assert_eq!(name.to_string(), "Ảnh chụp màn hình.png");
    }

    #[test]
    fn normalizes_to_nfc() {
        // "Ả" as A + U+0309 (combining hook above) must equal the composed form.
        let decomposed = parse("A\u{0309}nh.png").unwrap();
        let composed = parse("\u{1EA2}nh.png").unwrap();
        assert_eq!(decomposed, composed);
    }

    #[test]
    fn strips_unix_directory_components() {
        assert_eq!(
            parse("/var/www/photo.jpg").unwrap().to_string(),
            "photo.jpg"
        );
    }

    #[test]
    fn strips_windows_directory_components() {
        assert_eq!(
            parse(r"C:\Users\me\photo.jpg").unwrap().to_string(),
            "photo.jpg"
        );
    }

    #[test]
    fn traversal_loses_its_directory_part() {
        // `../../etc/passwd` sanitizes down to `passwd`, which has no
        // extension and is refused on that ground -- but the point is that
        // nothing above the base name survives to be a path.
        assert_eq!(
            parse("../../etc/passwd"),
            Err(FilenameError::MissingExtension)
        );
        assert_eq!(
            parse("../../etc/passwd.txt").unwrap().to_string(),
            "passwd.txt"
        );
    }

    #[test]
    fn rejects_bare_dot_segments() {
        assert_eq!(parse(".."), Err(FilenameError::Traversal));
        assert_eq!(parse("."), Err(FilenameError::Traversal));
    }

    #[test]
    fn rejects_a_nul_byte() {
        assert_eq!(parse("a\0b.jpg"), Err(FilenameError::ControlChar));
    }

    #[test]
    fn rejects_a_nul_hidden_in_the_directory_part() {
        assert_eq!(parse("../\0/photo.jpg"), Err(FilenameError::ControlChar));
    }

    #[test]
    fn rejects_a_newline() {
        assert_eq!(parse("a\r\nb.jpg"), Err(FilenameError::ControlChar));
    }

    #[test]
    fn rejects_a_right_to_left_override() {
        assert_eq!(
            parse("invoice\u{202E}gpj.txt"),
            Err(FilenameError::ControlChar)
        );
    }

    #[test]
    fn rejects_a_zero_width_joiner() {
        assert_eq!(parse("pho\u{200D}to.jpg"), Err(FilenameError::ControlChar));
    }

    #[test]
    fn rejects_a_unicode_tag_character() {
        // U+E0000-U+E007F render as nothing in every font, so two names that
        // differ only by a tag character are indistinguishable on screen.
        assert_eq!(parse("pho\u{E0041}to.jpg"), Err(FilenameError::ControlChar));
        assert_eq!(parse("photo\u{E007F}.jpg"), Err(FilenameError::ControlChar));
    }

    #[test]
    fn rejects_the_blank_rendering_separators() {
        for blank in ['\u{00AD}', '\u{180E}', '\u{3164}'] {
            let name = format!("pho{blank}to.jpg");
            assert_eq!(
                parse(&name),
                Err(FilenameError::ControlChar),
                "expected U+{:04X} to be fatal",
                blank as u32
            );
        }
    }

    #[test]
    fn keeps_a_variation_selector() {
        // A legitimate name can carry one beside an emoji; refusing it would
        // put this module back in the business of rejecting real filenames.
        let name = parse("heart\u{2764}\u{FE0F}.jpg").unwrap();
        assert_eq!(name.to_string(), "heart\u{2764}\u{FE0F}.jpg");
    }

    #[test]
    fn rejects_windows_device_names() {
        for name in ["CON.txt", "con.txt", "NUL.txt", "AUX.txt", "COM1.txt"] {
            assert_eq!(parse(name), Err(FilenameError::ReservedName), "{name}");
        }
    }

    #[test]
    fn rejects_a_device_name_behind_a_second_extension() {
        assert_eq!(parse("CON.foo.txt"), Err(FilenameError::ReservedName));
    }

    #[test]
    fn rejects_a_device_name_with_trailing_dots_and_spaces() {
        assert_eq!(parse("CON.txt. "), Err(FilenameError::ReservedName));
    }

    #[test]
    fn repairs_a_double_extension() {
        assert_eq!(parse("shell.php.jpg").unwrap().to_string(), "shell_php.jpg");
    }

    #[test]
    fn repairs_an_alternate_data_stream() {
        assert_eq!(
            parse("photo.jpg:payload.txt").unwrap().to_string(),
            "photo_jpg_payload.txt"
        );
    }

    #[test]
    fn rejects_an_extension_outside_the_whitelist() {
        for name in ["shell.php", "virus.exe", "page.phtml", "boot.svg"] {
            assert_eq!(
                parse(name),
                Err(FilenameError::ExtensionNotAllowed),
                "{name}"
            );
        }
    }

    #[test]
    fn rejects_a_missing_extension() {
        assert_eq!(parse("passwd"), Err(FilenameError::MissingExtension));
    }

    #[test]
    fn rejects_a_dotfile_with_no_stem() {
        assert_eq!(parse(".jpg"), Err(FilenameError::EmptyStem));
    }

    #[test]
    fn rejects_an_empty_name() {
        assert_eq!(parse(""), Err(FilenameError::Empty));
        assert_eq!(parse("   "), Err(FilenameError::Empty));
        assert_eq!(parse("/var/www/"), Err(FilenameError::Empty));
    }

    #[test]
    fn rejects_an_over_long_input() {
        let long = format!("{}.jpg", "a".repeat(MAX_INPUT_BYTES));
        assert_eq!(
            SafeFilename::parse(&long, &allowed()),
            Err(FilenameError::TooLong)
        );
    }

    #[test]
    fn truncates_a_long_stem_on_a_character_boundary() {
        let name = parse(&format!("{}.jpg", "ả".repeat(200))).unwrap();
        assert!(name.to_string().len() <= MAX_FILENAME_BYTES);
        // Truncation never splits a character.
        assert!(name.stem().chars().all(|ch| ch == 'ả'));
    }

    #[test]
    fn an_empty_whitelist_stores_nothing() {
        let empty = AllowedExtensions::default();
        assert!(empty.is_empty());
        assert_eq!(
            SafeFilename::parse("holiday.jpg", &empty),
            Err(FilenameError::ExtensionNotAllowed)
        );
    }

    #[test]
    fn extension_parsing_is_strict() {
        assert_eq!(Extension::parse(""), Err(FilenameError::MissingExtension));
        let too_long = "a".repeat(MAX_EXTENSION_BYTES + 1);
        for bad in ["jp g", "jpg.", "jpg/", "jpé", "j-pg", too_long.as_str()] {
            assert_eq!(
                Extension::parse(bad),
                Err(FilenameError::InvalidExtension),
                "{bad}"
            );
        }
        assert_eq!(Extension::parse("mp4").unwrap().as_str(), "mp4");
    }

    #[test]
    fn built_in_whitelists_are_what_they_say() {
        let images = AllowedExtensions::images();
        assert_eq!(images.len(), 5);
        assert!(!images.contains(&Extension::parse("svg").unwrap()));
        let documents = AllowedExtensions::documents();
        assert!(documents.contains(&Extension::parse("pdf").unwrap()));
    }
}
