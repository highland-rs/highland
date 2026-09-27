// Rust guideline compliant 2026-09-27

//! Loading a configuration from a file or a string.
//!
//! Loading is three separate steps so that a failure can be attributed
//! precisely: read the bytes, parse the document, then validate it
//! semantically (SPEC.md, §9.5).

use std::fs;
use std::path::Path;

use crate::error::ConfigError;
use crate::model::Config;
use crate::validation::{ValidationContext, validate};

/// The largest configuration file this release accepts (`L-06`).
pub const MAX_CONFIG_BYTES: usize = 4 * 1024 * 1024;

/// Parses a configuration document from TOML text.
///
/// # Errors
///
/// Returns [`ConfigError::Syntax`] when the text is not valid TOML and
/// [`ConfigError::Schema`] when it does not match the schema.
///
/// # Examples
///
/// ```
/// use highland_config::parse;
///
/// let text = r#"
/// schema_version = 1
/// [node]
/// name = "node-a"
/// "#;
/// let config = parse(text).expect("the document parses");
/// assert_eq!(config.node.name, "node-a");
/// ```
pub fn parse(text: &str) -> Result<Config, ConfigError> {
    toml::from_str(text).map_err(ConfigError::from)
}

/// Reads and parses a configuration file.
///
/// # Errors
///
/// Returns [`ConfigError::WorldWritable`] for a world-writable file unless
/// `allow_insecure` is set, [`ConfigError::TooLarge`] above
/// [`MAX_CONFIG_BYTES`], and the parse errors above otherwise.
///
/// # Examples
///
/// ```no_run
/// # use std::path::Path;
/// # fn main() -> Result<(), highland_config::ConfigError> {
/// let config = highland_config::load(Path::new("/etc/highland/config.toml"), false)?;
/// println!("{} instances", config.instances.len());
/// # Ok(())
/// # }
/// ```
pub fn load(path: &Path, allow_insecure: bool) -> Result<Config, ConfigError> {
    let bytes = read_bounded(path, allow_insecure)?;
    let text = String::from_utf8(bytes).map_err(|error| ConfigError::Schema {
        message: format!("configuration is not valid UTF-8: {error}"),
    })?;
    parse(&text)
}

fn read_bounded(path: &Path, allow_insecure: bool) -> Result<Vec<u8>, ConfigError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let metadata = fs::metadata(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        if usize::try_from(metadata.len()).unwrap_or(usize::MAX) > MAX_CONFIG_BYTES {
            return Err(ConfigError::TooLarge {
                path: path.to_path_buf(),
                size: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
                limit: MAX_CONFIG_BYTES,
            });
        }
        if !allow_insecure && metadata.permissions().mode() & 0o002 != 0 {
            return Err(ConfigError::WorldWritable {
                path: path.to_path_buf(),
            });
        }
    }
    #[cfg(not(unix))]
    let _ = allow_insecure;

    fs::read(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })
}

/// Parses, then validates, a configuration document.
///
/// # Errors
///
/// Returns every parse, schema, and semantic error found.
pub fn load_and_validate(
    text: &str,
    context: &ValidationContext<'_>,
) -> Result<Config, ConfigError> {
    let config = parse(text)?;
    validate(&config, context).map_err(|violations| ConfigError::Invalid { violations })?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temporary_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("highland-config-{name}-{}", std::process::id()))
    }

    #[test]
    fn a_world_writable_file_is_refused_unless_overridden() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            let path = temporary_path("world-writable");
            let mut file = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o666)
                .open(&path)
                .expect("the fixture can be created");
            file.write_all(b"[node]\nname = \"node-a\"\n")
                .expect("the fixture can be written");
            drop(file);
            fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o666))
                .expect("permissions can be set");

            assert!(matches!(
                load(&path, false),
                Err(ConfigError::WorldWritable { .. })
            ));
            assert!(matches!(load(&path, true), Err(ConfigError::Syntax { .. })));
            let _ = fs::remove_file(&path);
        }
    }

    #[test]
    fn a_missing_file_reports_the_path() {
        let path = temporary_path("absent");
        let _ = fs::remove_file(&path);
        assert!(matches!(load(&path, false), Err(ConfigError::Read { .. })));
    }

    #[test]
    fn a_file_above_the_limit_is_refused() {
        let path = temporary_path("too-large");
        let file = fs::File::create(&path).expect("the fixture can be created");
        file.set_len(u64::try_from(MAX_CONFIG_BYTES).expect("the limit fits in a u64") + 1)
            .expect("the fixture can be extended");
        drop(file);

        assert!(matches!(
            load(&path, true),
            Err(ConfigError::TooLarge { .. })
        ));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn the_size_limit_is_four_mebibytes() {
        assert_eq!(MAX_CONFIG_BYTES, 4_194_304);
    }
}
