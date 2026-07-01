//! A tiny, read-only GGUF metadata reader — just enough to learn a model's
//! native context window (`*.context_length`) from the file on disk.
//!
//! We don't pull in a full GGUF crate: we only need one integer key, and the
//! header sits at the very start of the file, so a streaming scan of the
//! metadata block (stopping as soon as the key is found) reads a few MB at most
//! and never touches the multi-gigabyte tensor data.
//!
//! Reference: the GGUF format spec (v2/v3). Counts and string lengths are
//! little-endian `u64`; we don't support the extinct v1 (`u32`) layout.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

const MAGIC: &[u8; 4] = b"GGUF";
/// Refuse absurd lengths so a corrupt/hostile header can't make us read forever.
const MAX_LEN: u64 = 64 * 1024 * 1024;
/// A model's metadata never has this many keys; guards against a bogus count.
const MAX_KV: u64 = 1 << 20;

// GGUF metadata value types.
const T_UINT8: u32 = 0;
const T_INT8: u32 = 1;
const T_UINT16: u32 = 2;
const T_INT16: u32 = 3;
const T_UINT32: u32 = 4;
const T_INT32: u32 = 5;
const T_FLOAT32: u32 = 6;
const T_BOOL: u32 = 7;
const T_STRING: u32 = 8;
const T_ARRAY: u32 = 9;
const T_UINT64: u32 = 10;
const T_INT64: u32 = 11;
const T_FLOAT64: u32 = 12;

/// Read the native (training) context length from a GGUF file's metadata, e.g.
/// `qwen2.context_length`. Returns `None` if the file isn't a readable GGUF, the
/// key is absent, or anything about the header doesn't parse — callers treat
/// that as "unknown" and fall back to a conservative default.
pub fn context_length(path: &Path) -> Option<u32> {
    let file = File::open(path).ok()?;
    let mut r = BufReader::new(file);
    read_context_length(&mut r).ok().flatten()
}

fn read_context_length<R: Read>(r: &mut R) -> io::Result<Option<u32>> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Ok(None);
    }
    let version = read_u32(r)?;
    if version < 2 {
        // v1 used 32-bit counts/lengths; not worth supporting.
        return Ok(None);
    }
    let _tensor_count = read_u64(r)?;
    let kv_count = read_u64(r)?;
    if kv_count > MAX_KV {
        return Ok(None);
    }

    for _ in 0..kv_count {
        let key = read_string(r)?;
        let vtype = read_u32(r)?;
        // The architecture prefix varies (llama/qwen2/…), so match on the suffix.
        if key.ends_with(".context_length") {
            return Ok(read_u32_value(r, vtype)?);
        }
        skip_value(r, vtype)?;
    }
    Ok(None)
}

/// Read a scalar integer metadata value as a `u32`. Returns `None` for value
/// types that can't be a context length (so we keep scanning rather than fail).
fn read_u32_value<R: Read>(r: &mut R, vtype: u32) -> io::Result<Option<u32>> {
    let v = match vtype {
        T_UINT32 => read_u32(r)?,
        T_INT32 => read_u32(r)?, // context lengths are non-negative; reinterpret.
        T_UINT64 | T_INT64 => read_u64(r)?.try_into().unwrap_or(u32::MAX),
        T_UINT16 | T_INT16 => read_u16(r)? as u32,
        _ => {
            skip_value(r, vtype)?;
            return Ok(None);
        }
    };
    Ok(Some(v))
}

/// Consume a metadata value of `vtype` without retaining it. Reads sequentially
/// (no seeking) so it works on any reader and stays buffer-friendly.
fn skip_value<R: Read>(r: &mut R, vtype: u32) -> io::Result<()> {
    match vtype {
        T_UINT8 | T_INT8 | T_BOOL => skip_bytes(r, 1),
        T_UINT16 | T_INT16 => skip_bytes(r, 2),
        T_UINT32 | T_INT32 | T_FLOAT32 => skip_bytes(r, 4),
        T_UINT64 | T_INT64 | T_FLOAT64 => skip_bytes(r, 8),
        T_STRING => {
            let n = read_u64(r)?;
            skip_bytes(r, n)
        }
        T_ARRAY => {
            let elem_type = read_u32(r)?;
            let count = read_u64(r)?;
            if let Some(size) = fixed_size(elem_type) {
                skip_bytes(r, count.saturating_mul(size))
            } else {
                // Variable-width elements (strings, nested arrays): skip each.
                for _ in 0..count {
                    skip_value(r, elem_type)?;
                }
                Ok(())
            }
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown GGUF value type {vtype}"),
        )),
    }
}

/// Byte width of a fixed-size scalar value type, or `None` if variable-width.
fn fixed_size(vtype: u32) -> Option<u64> {
    match vtype {
        T_UINT8 | T_INT8 | T_BOOL => Some(1),
        T_UINT16 | T_INT16 => Some(2),
        T_UINT32 | T_INT32 | T_FLOAT32 => Some(4),
        T_UINT64 | T_INT64 | T_FLOAT64 => Some(8),
        _ => None,
    }
}

fn skip_bytes<R: Read>(r: &mut R, n: u64) -> io::Result<()> {
    if n > MAX_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "GGUF field length implausibly large",
        ));
    }
    let copied = io::copy(&mut r.take(n), &mut io::sink())?;
    if copied != n {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short GGUF read"));
    }
    Ok(())
}

fn read_string<R: Read>(r: &mut R) -> io::Result<String> {
    let n = read_u64(r)?;
    if n > MAX_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "GGUF string length implausibly large",
        ));
    }
    let mut buf = vec![0u8; n as usize];
    r.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn read_u16<R: Read>(r: &mut R) -> io::Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}

fn read_u32<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64<R: Read>(r: &mut R) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build a minimal in-memory GGUF with the given metadata KV entries.
    /// Each entry is (key, value-type, value-bytes-little-endian).
    fn gguf(kvs: &[(&str, u32, Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&3u32.to_le_bytes()); // version
        out.extend_from_slice(&0u64.to_le_bytes()); // tensor_count
        out.extend_from_slice(&(kvs.len() as u64).to_le_bytes());
        for (key, vtype, value) in kvs {
            out.extend_from_slice(&(key.len() as u64).to_le_bytes());
            out.extend_from_slice(key.as_bytes());
            out.extend_from_slice(&vtype.to_le_bytes());
            out.extend_from_slice(value);
        }
        out
    }

    fn string_val(s: &str) -> Vec<u8> {
        let mut v = (s.len() as u64).to_le_bytes().to_vec();
        v.extend_from_slice(s.as_bytes());
        v
    }

    #[test]
    fn reads_context_length_after_other_keys() {
        let bytes = gguf(&[
            ("general.architecture", T_STRING, string_val("qwen2")),
            ("qwen2.block_count", T_UINT32, 48u32.to_le_bytes().to_vec()),
            ("qwen2.context_length", T_UINT32, 1_048_576u32.to_le_bytes().to_vec()),
        ]);
        let got = read_context_length(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(got, Some(1_048_576));
    }

    #[test]
    fn skips_arrays_to_reach_the_key() {
        // A string array (like the tokenizer vocab) must be skipped correctly.
        let mut arr = T_STRING.to_le_bytes().to_vec();
        arr.extend_from_slice(&2u64.to_le_bytes());
        arr.extend_from_slice(&string_val("a"));
        arr.extend_from_slice(&string_val("bb"));
        let bytes = gguf(&[
            ("tokenizer.ggml.tokens", T_ARRAY, arr),
            ("llama.context_length", T_UINT64, 32_768u64.to_le_bytes().to_vec()),
        ]);
        let got = read_context_length(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(got, Some(32_768));
    }

    #[test]
    fn missing_key_returns_none() {
        let bytes = gguf(&[("general.name", T_STRING, string_val("nope"))]);
        assert_eq!(read_context_length(&mut Cursor::new(bytes)).unwrap(), None);
    }

    #[test]
    fn non_gguf_returns_none() {
        let got = read_context_length(&mut Cursor::new(b"not a gguf file".to_vec())).unwrap();
        assert_eq!(got, None);
    }
}
