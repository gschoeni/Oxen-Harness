//! Helpers for reading typed values out of a model-provided JSON arguments
//! object. Every tool's `invoke` receives a `serde_json::Value`, and these keep
//! the extraction terse, consistent, and uniformly error-messaged.

use crate::ToolError;

/// Read a required string argument, erroring if it is missing or not a string.
pub(crate) fn require_str<'a>(
    args: &'a serde_json::Value,
    key: &str,
) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArguments(format!("missing string `{key}`")))
}

/// Read an optional string argument.
pub(crate) fn opt_str<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

/// Read an optional `u64` argument.
pub(crate) fn opt_u64(args: &serde_json::Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}

/// Read an optional count argument as `usize`, falling back to `default`.
pub(crate) fn opt_usize(args: &serde_json::Value, key: &str, default: usize) -> usize {
    opt_u64(args, key).map_or(default, |n| n as usize)
}

/// Read an optional boolean argument, defaulting to `false`.
pub(crate) fn opt_bool(args: &serde_json::Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}
