//! Format-neutral waveform format detection.
//!
//! This module deliberately contains only the detection seam. VCD-specific
//! APIs remain in the root module and third-party FST reader types do not
//! appear in this public API.

use std::fmt;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use fst_reader::{FstReader, is_fst_file};
use thiserror::Error;

/// Maximum input size accepted by the lightweight public detection helper.
///
/// This is not the eventual server/open resource policy. It prevents an
/// accidental or hostile path from making format detection inspect an
/// effectively unbounded file before `OpenedWaveform` exists.
const MAX_DETECTION_INPUT_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const FST_GZIP_WRAPPER_BLOCK: u8 = 254;
const MAX_ERROR_MESSAGE_BYTES: usize = 512;

/// A waveform file format supported by the generic waveform APIs.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaveformFormat {
    Vcd,
    Fst,
}

impl fmt::Display for WaveformFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Vcd => "vcd",
            Self::Fst => "fst",
        })
    }
}

/// An optional format assertion for content-based waveform detection.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WaveformFormatHint {
    #[default]
    Auto,
    Vcd,
    Fst,
}

impl From<WaveformFormat> for WaveformFormatHint {
    fn from(format: WaveformFormat) -> Self {
        match format {
            WaveformFormat::Vcd => Self::Vcd,
            WaveformFormat::Fst => Self::Fst,
        }
    }
}

/// Failure while identifying or validating a waveform input.
#[derive(Debug, Error)]
pub enum WaveformDetectionError {
    #[error("failed to read waveform {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("{requested} format was requested, but the input is valid {detected}")]
    ExplicitMismatch {
        requested: WaveformFormat,
        detected: WaveformFormat,
    },
    #[error("input is not a valid {format} waveform: {message}")]
    Invalid {
        format: WaveformFormat,
        message: String,
    },
    #[error("unsupported FST input: {message}")]
    UnsupportedFst { message: String },
    #[error("waveform input exceeds the format-detection limit ({actual} > {limit} bytes)")]
    ResourceLimit { actual: u64, limit: u64 },
    #[error("FST parser failed while validating the input")]
    FstParserPanicked,
    #[error("could not detect a VCD or FST waveform: {message}")]
    Undetected { message: String },
}

fn io_error(path: &Path, source: io::Error) -> WaveformDetectionError {
    WaveformDetectionError::Io {
        path: path.display().to_string(),
        source,
    }
}

fn bounded_message(message: impl fmt::Display) -> String {
    let message = message.to_string();
    if message.len() <= MAX_ERROR_MESSAGE_BYTES {
        return message;
    }
    let mut end = MAX_ERROR_MESSAGE_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &message[..end])
}

/// Probe and validate FST from one opened file generation.
///
/// `Ok(false)` means that the file does not have FST structure. Whole-file
/// gzip wrappers are rejected before `fst-reader` can decompress them into
/// memory. Parser panics are contained at this public input boundary.
fn probe_and_validate_fst(path: &Path) -> Result<bool, WaveformDetectionError> {
    let mut file = File::open(path).map_err(|error| io_error(path, error))?;
    let length = file
        .metadata()
        .map_err(|error| io_error(path, error))?
        .len();
    if length > MAX_DETECTION_INPUT_BYTES {
        return Err(WaveformDetectionError::ResourceLimit {
            actual: length,
            limit: MAX_DETECTION_INPUT_BYTES,
        });
    }

    let mut first = [0u8; 1];
    let bytes_read = file
        .read(&mut first)
        .map_err(|error| io_error(path, error))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| io_error(path, error))?;
    if bytes_read == 1 && first[0] == FST_GZIP_WRAPPER_BLOCK {
        return Err(WaveformDetectionError::UnsupportedFst {
            message: "whole-file gzip wrappers are disabled because fst-reader decompresses them entirely in memory"
                .to_string(),
        });
    }

    let looks_like_fst = catch_unwind(AssertUnwindSafe(|| is_fst_file(&mut file)))
        .map_err(|_| WaveformDetectionError::FstParserPanicked)?;
    if !looks_like_fst {
        return Ok(false);
    }

    file.seek(SeekFrom::Start(0))
        .map_err(|error| io_error(path, error))?;
    catch_unwind(AssertUnwindSafe(|| FstReader::open(BufReader::new(file))))
        .map_err(|_| WaveformDetectionError::FstParserPanicked)?
        .map(|_| true)
        .map_err(|error| WaveformDetectionError::Invalid {
            format: WaveformFormat::Fst,
            message: bounded_message(error),
        })
}

fn validate_vcd(path: &Path) -> Result<(), WaveformDetectionError> {
    crate::opened::OpenedVcd::open(path)
        .map(|_| ())
        .map_err(|error| WaveformDetectionError::Invalid {
            format: WaveformFormat::Vcd,
            message: bounded_message(error),
        })
}

/// Detect and validate the format of one waveform file.
///
/// Detection is based on file contents, never the filename extension. The FST
/// probe and validation operate on one opened file generation. This function
/// returns classification only: code that subsequently consumes the file must
/// still open and validate its own generation. The future `OpenedWaveform`
/// facade will combine classification and opening atomically.
///
/// An explicit hint validates that format and returns
/// [`WaveformDetectionError::ExplicitMismatch`] when the other supported
/// format is valid. Empty and truncated inputs are rejected rather than being
/// guessed as VCD or FST.
pub fn detect_waveform_format(
    path: impl AsRef<Path>,
    hint: WaveformFormatHint,
) -> Result<WaveformFormat, WaveformDetectionError> {
    let path = path.as_ref();
    let looks_like_fst = probe_and_validate_fst(path)?;

    if looks_like_fst {
        if matches!(hint, WaveformFormatHint::Vcd) {
            return Err(WaveformDetectionError::ExplicitMismatch {
                requested: WaveformFormat::Vcd,
                detected: WaveformFormat::Fst,
            });
        }
        return Ok(WaveformFormat::Fst);
    }

    if matches!(hint, WaveformFormatHint::Fst) {
        if validate_vcd(path).is_ok() {
            return Err(WaveformDetectionError::ExplicitMismatch {
                requested: WaveformFormat::Fst,
                detected: WaveformFormat::Vcd,
            });
        }
        return Err(WaveformDetectionError::Invalid {
            format: WaveformFormat::Fst,
            message: "FST signature not found".to_string(),
        });
    }

    match validate_vcd(path) {
        Ok(()) => Ok(WaveformFormat::Vcd),
        Err(WaveformDetectionError::Invalid { message, .. }) => {
            if matches!(hint, WaveformFormatHint::Vcd) {
                Err(WaveformDetectionError::Invalid {
                    format: WaveformFormat::Vcd,
                    message,
                })
            } else {
                Err(WaveformDetectionError::Undetected { message })
            }
        }
        Err(error) => Err(error),
    }
}
