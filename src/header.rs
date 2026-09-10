use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use vcd::Parser;

use crate::catalog::SignalCatalog;
use crate::opened::{FileIdentity, OpenOptions};
use crate::{Result, Timescale, VcdError};

pub(crate) struct CompactHeader {
    pub(crate) catalog: SignalCatalog,
    pub(crate) timescale: Option<Timescale>,
    pub(crate) body_offset: u64,
}

#[allow(dead_code)] // Consumed by OpenedVcd::open in RQ-M1-T04.
pub(crate) struct IdentifiedCompactHeader {
    pub(crate) header: CompactHeader,
    pub(crate) identity: FileIdentity,
}

pub(crate) fn sanitize_header(header: &[u8]) -> String {
    fn needs_escape(ident: &str) -> bool {
        if ident.starts_with('\\') || ident.is_empty() {
            return false;
        }
        let starts_ok = ident
            .chars()
            .next()
            .map(|ch| ch.is_ascii_alphabetic() || ch == '_')
            .unwrap_or(false);
        if !starts_ok {
            return true;
        }
        let allowed_chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_$().";
        ident.chars().any(|ch| !allowed_chars.contains(ch))
    }

    let text = String::from_utf8_lossy(header);
    let mut sanitized = String::with_capacity(header.len());

    for line in text.split_inclusive(['\n', '\r']) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("$scope ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 && parts[0] == "$scope" {
                let ident = parts[2];
                let fixed = if needs_escape(ident) {
                    if ident.starts_with('\\') {
                        ident.to_string()
                    } else {
                        format!("\\{ident}")
                    }
                } else {
                    ident.to_string()
                };
                let newline = if line.ends_with('\n') { "\n" } else { "" };
                let prefix_len = line.len() - trimmed.len();
                let prefix = &line[..prefix_len];
                let rebuilt = format!("{prefix}$scope {} {fixed} $end{newline}", parts[1]);
                sanitized.push_str(&rebuilt);
                continue;
            }
        }
        sanitized.push_str(line);
    }

    sanitized
}

pub(crate) fn read_header_bytes_from_file(file: &mut File) -> Result<(Vec<u8>, u64)> {
    file.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::new(file);
    let mut header_bytes = Vec::new();
    let mut line_buf = Vec::new();
    let mut found = false;

    loop {
        line_buf.clear();
        let bytes = reader.read_until(b'\n', &mut line_buf)?;
        if bytes == 0 {
            break;
        }
        header_bytes.extend_from_slice(&line_buf);
        if line_buf
            .windows(b"$enddefinitions".len())
            .any(|window| window == b"$enddefinitions")
        {
            found = true;
            break;
        }
    }

    if !found {
        return Err(VcdError::MissingEndDefinitions);
    }

    let body_offset = reader.stream_position()?;
    // BufReader may have read ahead. Align the underlying provenance handle to
    // the logical boundary before returning it to identity/opened-file code.
    reader.seek(SeekFrom::Start(body_offset))?;
    Ok((header_bytes, body_offset))
}

pub(crate) fn read_header_bytes(path: &Path) -> Result<(Vec<u8>, u64)> {
    let mut file = File::open(path)?;
    read_header_bytes_from_file(&mut file)
}

pub(crate) fn parse_compact_header(bytes: &[u8], body_offset: u64) -> Result<CompactHeader> {
    let sanitized = sanitize_header(bytes);
    let cursor = io::Cursor::new(sanitized.as_bytes());
    let mut parser = Parser::new(cursor);
    let mut header = parser
        .parse_header()
        .map_err(|error| VcdError::Parse(error.to_string()))?;

    let timescale = header
        .timescale
        .map(|(magnitude, unit)| Timescale { magnitude, unit });
    // vcd 0.7 necessarily materializes Header/ScopeItem first. Move its item
    // tree into the compact builder and drop the remaining Header metadata
    // before conversion. The builder then consumes processed tree nodes.
    let items = std::mem::take(&mut header.items);
    drop(header);
    let catalog = SignalCatalog::from_scope_items(items)?;
    Ok(CompactHeader {
        catalog,
        timescale,
        body_offset,
    })
}

pub(crate) fn read_compact_header(path: impl AsRef<Path>) -> Result<CompactHeader> {
    let (header_bytes, body_offset) = read_header_bytes(path.as_ref())?;
    parse_compact_header(&header_bytes, body_offset)
}

#[allow(dead_code)] // Called by OpenedVcd::open in RQ-M1-T04.
pub(crate) fn read_identified_compact_header(
    path: impl AsRef<Path>,
    options: &OpenOptions,
) -> Result<IdentifiedCompactHeader> {
    let mut file = File::open(path)?;
    let metadata_before_header = file.metadata()?;
    let (header_bytes, body_offset) = read_header_bytes_from_file(&mut file)?;
    let identity = FileIdentity::from_open_file(
        &mut file,
        &header_bytes,
        body_offset,
        options.fingerprint_policy(),
    )?;
    if !identity.matches_metadata(&metadata_before_header) {
        return Err(io::Error::other("VCD changed while reading its header").into());
    }
    let header = parse_compact_header(&header_bytes, body_offset)?;
    Ok(IdentifiedCompactHeader { header, identity })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_header_retains_body_offset_and_timescale() {
        let parsed = read_compact_header("tests/fixtures/query_semantics.vcd").expect("header");
        assert!(parsed.body_offset > 0);
        assert_eq!(parsed.catalog.len(), 7);
        assert_eq!(parsed.timescale.expect("timescale").magnitude, 1);
    }

    #[test]
    fn compact_header_preserves_sanitized_scope_spelling() {
        let parsed = read_compact_header("tests/fixtures/crlf_unusual_header.vcd").expect("header");
        assert_eq!(
            parsed.catalog.names().collect::<Vec<_>>(),
            ["\\top-scope.sig"]
        );
    }

    #[test]
    fn identified_header_uses_one_handle_for_offset_and_fingerprints() {
        let parsed = read_identified_compact_header(
            "tests/fixtures/query_semantics.vcd",
            &OpenOptions::new(),
        )
        .expect("identified header");
        assert_eq!(parsed.header.body_offset, parsed.identity.body_offset());
        assert_eq!(parsed.header.catalog.len(), 7);
        assert!(parsed.identity.len() > parsed.identity.body_offset());
        assert!(parsed.identity.full_content_fingerprint().is_none());
    }

    #[test]
    fn parse_uses_supplied_exact_body_offset() {
        let bytes = b"$timescale 1 ns $end\n$enddefinitions $end\n";
        let parsed = parse_compact_header(bytes, 1234).expect("header");
        assert_eq!(parsed.body_offset, 1234);
    }
}
