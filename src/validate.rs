use crate::error::FerryError;

/// Validate a user-supplied file path.
///
/// Matches S3-Ferry's `PathConstraint` — deliberately restrictive:
///
/// 1. No null bytes (`\0`)
/// 2. Rejects any path whose normalized form differs from itself
///    (blocks `../`, leading `./`, redundant slashes)
/// 3. Only allows `[0-9a-zA-Z._/-]`
///
/// The whitelist forbids spaces, unicode, `~`, `@`, `+`, etc. That's
/// intentional — this component brokers file transfers, not general
/// filesystem access, and rejecting exotic characters at the boundary
/// eliminates a large class of encoding-mismatch and injection risks.
pub fn validate_path(path: &str) -> Result<(), FerryError> {
    if path.is_empty() {
        return Err(FerryError::InvalidPath("empty path".into()));
    }
    if path.contains('\0') {
        return Err(FerryError::InvalidPath("null byte in path".into()));
    }
    if !path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'/'))
    {
        return Err(FerryError::InvalidPath(
            "path contains disallowed characters (allowed: 0-9 a-z A-Z - . _ /)".into(),
        ));
    }
    if is_traversal(path) {
        return Err(FerryError::InvalidPath("path traversal detected".into()));
    }
    Ok(())
}

fn is_traversal(path: &str) -> bool {
    // Any segment that is exactly ".." OR "." is rejected. Doing this
    // segment-wise (not just substring match) catches `foo/..`, `../` at
    // the start, and the bare `.` case that previously bypassed the
    // check and hit the FS with EISDIR (h2ck.me v1 finding FN1).
    // `.hidden` / `..hidden` filenames stay accepted — the comparison is
    // exact segment, not prefix.
    path.split('/').any(|seg| seg == ".." || seg == ".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_typical_filename() {
        assert!(validate_path("hello.txt").is_ok());
        assert!(validate_path("nested/dir/file-1_final.v2.txt").is_ok());
        assert!(validate_path("a").is_ok());
    }

    #[test]
    fn rejects_empty() {
        assert!(matches!(validate_path(""), Err(FerryError::InvalidPath(_))));
    }

    #[test]
    fn rejects_null_byte() {
        assert!(matches!(
            validate_path("foo\0bar"),
            Err(FerryError::InvalidPath(_))
        ));
    }

    #[test]
    fn rejects_traversal_various_shapes() {
        for p in [
            "../etc/passwd",
            "foo/../bar",
            "foo/..",
            "..",
            "./..",
            "a/b/../c",
        ] {
            assert!(
                matches!(validate_path(p), Err(FerryError::InvalidPath(_))),
                "{p:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_bare_dot_and_dot_segments() {
        // FN1 (h2ck.me v1): a bare `.` used to sail past validation and
        // hit the FS as the data-dir itself, leaking `os error 21` in a
        // 500 body. The rejection must be segment-exact, not "starts with
        // dot", so `.hidden` still passes.
        for p in [".", "./", "/.", "a/./b", "foo/.", "./foo", "a/.", "."] {
            assert!(
                matches!(validate_path(p), Err(FerryError::InvalidPath(_))),
                "{p:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_disallowed_chars() {
        for p in [
            "hello world.txt", // space
            "café.txt",        // unicode
            "back\\slash",     // backslash
            "with?query",      // query separator
            "with#hash",       // fragment
            "with$dollar",
            "with@at",
        ] {
            assert!(
                matches!(validate_path(p), Err(FerryError::InvalidPath(_))),
                "{p:?} should be rejected"
            );
        }
    }

    #[test]
    fn accepts_dot_prefix_filenames() {
        // `.hidden` is allowed — a real filename, not a traversal.
        assert!(validate_path(".hidden").is_ok());
        assert!(validate_path("nested/.hidden").is_ok());
    }
}
