//! Datasets — windowed, sortable, searchable reads over CSV/TSV/JSONL/Parquet
//! files, so the Editor pane's data grid can page through million-row files
//! without ever materializing them in the webview. A query ships one small
//! window of rows over IPC; slicing and sorting push down into Polars' lazy
//! engine, small files are cached in memory keyed by mtime, and big ones
//! stream from disk per request. Cell edits are surgical: CSV/JSONL rewrite
//! only the touched record (every other byte preserved, so diffs stay
//! reviewable), Parquet rewrites the whole file behind a size cap.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use polars::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use super::files::resolve;

/// Files at or under this size are parsed once and kept as an in-memory
/// DataFrame, so paging and re-sorting are instant. Bigger files re-scan
/// lazily on every request instead of occupying RAM.
const CACHE_MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
/// How many parsed files to keep around (least-recently-used beyond this).
const CACHE_MAX_ENTRIES: usize = 4;
/// Parquet edits rewrite the whole file, so cap how big a file we'll edit.
const PARQUET_MAX_EDIT_BYTES: u64 = 256 * 1024 * 1024;
/// Hard ceiling on rows per page, whatever the frontend asks for.
const MAX_PAGE_ROWS: u64 = 1_000;

/// Physical-row-index column injected before filter/sort, so every row the
/// grid shows knows which record in the file it is (edits address that).
const ROW_ID: &str = "__oxh_row_id__";

/// `Number.MAX_SAFE_INTEGER` (2^53 − 1): the largest integer JSON can hand a
/// JS grid without rounding.
const MAX_JS_SAFE_INT: u64 = 9_007_199_254_740_991;

// ---- request / response shapes ------------------------------------------------

/// One page request from the grid.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DatasetQueryReq {
    /// First row of the window, in the current view order.
    pub offset: u64,
    /// Rows in the window (capped at [`MAX_PAGE_ROWS`]).
    pub limit: u64,
    /// Column to sort by (view order = file order when absent).
    pub sort_by: Option<String>,
    #[serde(default)]
    pub descending: bool,
    /// Case-insensitive substring match across all columns.
    pub search: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ColumnMeta {
    pub name: String,
    /// The Polars dtype, verbatim (shown in the column header tooltip).
    pub dtype: String,
    /// Simplified family the UI keys icons and alignment off:
    /// int | float | bool | str | date | datetime | time | duration | list | struct | other
    pub kind: &'static str,
}

/// One window of a dataset, plus everything the grid chrome needs.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DatasetPage {
    pub columns: Vec<ColumnMeta>,
    /// Cell values, row-major, JSON-typed. Temporal/nested values arrive as
    /// display strings.
    pub rows: Vec<Vec<Json>>,
    /// Physical file record index of each row in `rows` (edit addressing).
    pub row_ids: Vec<u64>,
    /// Rows in the current view (after search), not just this window.
    pub total_rows: u64,
    pub file_size: u64,
    pub format: &'static str,
    pub elapsed_ms: u64,
    pub editable: bool,
    /// File mtime when this page was read; echoed back on writes so an edit
    /// against a file that changed underneath is refused, not misapplied.
    pub mtime_ms: u64,
}

/// The file's mtime in ms since the epoch — the edit-staleness token.
fn file_mtime_ms(meta: &fs::Metadata) -> Result<u64, String> {
    let modified = meta.modified().map_err(|e| e.to_string())?;
    Ok(modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0))
}

// ---- formats -------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Csv,
    Tsv,
    Jsonl,
    Parquet,
}

impl Format {
    fn from_path(path: &Path) -> Option<Format> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "csv" => Some(Format::Csv),
            "tsv" => Some(Format::Tsv),
            "jsonl" | "ndjson" => Some(Format::Jsonl),
            "parquet" => Some(Format::Parquet),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Tsv => "tsv",
            Format::Jsonl => "jsonl",
            Format::Parquet => "parquet",
        }
    }

    fn delimiter(self) -> u8 {
        if self == Format::Tsv {
            b'\t'
        } else {
            b','
        }
    }
}

/// A lazy scan of the file — nothing is read until collect, and Polars pushes
/// the slice down so unsorted paging never parses the whole file.
fn lazy_source(file: &Path, format: Format) -> Result<LazyFrame, String> {
    let source = PlRefPath::new(file.to_string_lossy().as_ref());
    let scan = match format {
        Format::Csv | Format::Tsv => LazyCsvReader::new(source)
            .with_separator(format.delimiter())
            .with_has_header(true)
            .with_try_parse_dates(true)
            .with_infer_schema_length(Some(1000))
            .finish(),
        Format::Jsonl => LazyJsonLineReader::new(source)
            .with_infer_schema_length(std::num::NonZeroUsize::new(1000))
            .finish(),
        Format::Parquet => LazyFrame::scan_parquet(source, ScanArgsParquet::default()),
    };
    scan.map_err(|e| format!("could not open {}: {e}", file.display()))
}

// ---- the parsed-file cache ------------------------------------------------------

struct CacheEntry {
    mtime: SystemTime,
    size: u64,
    df: DataFrame,
    last_used: Instant,
}

/// One counted view: this file, at this mtime, filtered by this search.
type ViewKey = (PathBuf, SystemTime, String);

/// Parsed small files, keyed by absolute path and invalidated by mtime+size.
/// Cheap to clone (the maps are shared), so commands can move a handle into a
/// blocking task.
#[derive(Default, Clone)]
pub(crate) struct DatasetCache {
    entries: Arc<Mutex<HashMap<PathBuf, CacheEntry>>>,
    /// View row counts — counting a >128 MB file is a full scan, and every
    /// page of the same view would repay it.
    counts: Arc<Mutex<HashMap<ViewKey, u64>>>,
    /// Serializes cell edits: two concurrent splices would compute byte
    /// offsets against different generations of the file and corrupt it.
    edit_lock: Arc<Mutex<()>>,
}

impl DatasetCache {
    /// The file as an in-memory frame if it's small enough — parsing it now
    /// if needed — or `None` when it should stream lazily instead.
    fn frame(
        &self,
        file: &Path,
        format: Format,
        size: u64,
        mtime: SystemTime,
    ) -> Result<Option<DataFrame>, String> {
        if size > CACHE_MAX_FILE_BYTES {
            return Ok(None);
        }
        if let Some(entry) = self.entries.lock().unwrap().get_mut(file) {
            if entry.mtime == mtime && entry.size == size {
                entry.last_used = Instant::now();
                return Ok(Some(entry.df.clone()));
            }
        }
        let df = lazy_source(file, format)?
            .collect()
            .map_err(|e| format!("could not parse {}: {e}", file.display()))?;
        let mut entries = self.entries.lock().unwrap();
        if entries.len() >= CACHE_MAX_ENTRIES && !entries.contains_key(file) {
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(p, _)| p.clone())
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(
            file.to_path_buf(),
            CacheEntry {
                mtime,
                size,
                df: df.clone(),
                last_used: Instant::now(),
            },
        );
        Ok(Some(df))
    }

    fn invalidate(&self, file: &Path) {
        self.entries.lock().unwrap().remove(file);
        self.counts.lock().unwrap().retain(|(p, _, _), _| p != file);
    }

    /// Rows in the view, computing (and remembering) it on first sight of
    /// this (file, mtime, search) combination.
    fn total_rows(
        &self,
        file: &Path,
        mtime: SystemTime,
        search: &str,
        view: &LazyFrame,
        path: &str,
    ) -> Result<u64, String> {
        let key = (file.to_path_buf(), mtime, search.to_string());
        if let Some(n) = self.counts.lock().unwrap().get(&key) {
            return Ok(*n);
        }
        let n = view
            .clone()
            .select([len().alias("len")])
            .collect()
            .map_err(|e| format!("could not count rows of {path}: {e}"))?
            .column("len")
            .and_then(|c| c.get(0))
            .map(|av| av.extract::<u64>().unwrap_or(0))
            .map_err(|e| e.to_string())?;
        let mut counts = self.counts.lock().unwrap();
        if counts.len() >= 64 {
            counts.clear();
        }
        counts.insert(key, n);
        Ok(n)
    }
}

// ---- querying -------------------------------------------------------------------

fn dtype_kind(dt: &DataType) -> &'static str {
    match dt {
        DataType::Boolean => "bool",
        dt if dt.is_integer() => "int",
        dt if dt.is_float() => "float",
        DataType::String | DataType::Categorical(_, _) | DataType::Enum(_, _) => "str",
        DataType::Date => "date",
        DataType::Datetime(_, _) => "datetime",
        DataType::Time => "time",
        DataType::Duration(_) => "duration",
        DataType::List(_) | DataType::Array(_, _) => "list",
        DataType::Struct(_) => "struct",
        _ => "other",
    }
}

fn json_value(av: AnyValue) -> Json {
    match av {
        AnyValue::Null => Json::Null,
        AnyValue::Boolean(b) => Json::Bool(b),
        AnyValue::String(s) => Json::String(s.to_string()),
        AnyValue::StringOwned(s) => Json::String(s.to_string()),
        AnyValue::Int8(v) => Json::from(v),
        AnyValue::Int16(v) => Json::from(v),
        AnyValue::Int32(v) => Json::from(v),
        // Past 2^53 a JS number silently rounds (snowflake ids are the classic
        // case), so wide integers ship as strings and stay exact in the grid.
        AnyValue::Int64(v) if v.unsigned_abs() <= MAX_JS_SAFE_INT => Json::from(v),
        AnyValue::Int64(v) => Json::String(v.to_string()),
        AnyValue::UInt8(v) => Json::from(v),
        AnyValue::UInt16(v) => Json::from(v),
        AnyValue::UInt32(v) => Json::from(v),
        AnyValue::UInt64(v) if v <= MAX_JS_SAFE_INT => Json::from(v),
        AnyValue::UInt64(v) => Json::String(v.to_string()),
        AnyValue::Float32(v) => serde_json::Number::from_f64(f64::from(v))
            .map(Json::Number)
            .unwrap_or(Json::Null),
        AnyValue::Float64(v) => serde_json::Number::from_f64(v)
            .map(Json::Number)
            .unwrap_or(Json::Null),
        // Temporal, nested, and exotic values ship as their display strings —
        // the grid shows them read-only rather than round-tripping them.
        other => Json::String(other.to_string()),
    }
}

/// Case-insensitive "any column contains" filter. Columns that can't cast to
/// a string (nested types) don't participate — and neither does the injected
/// row index, which isn't data.
fn search_expr(schema: &Schema, needle: &str) -> Option<Expr> {
    let needle = needle.to_lowercase();
    schema
        .iter()
        .filter(|(name, dt)| {
            name.as_str() != ROW_ID
                && !matches!(
                    dt,
                    DataType::List(_) | DataType::Array(_, _) | DataType::Struct(_)
                )
        })
        .map(|(name, _)| {
            col(name.clone())
                .cast(DataType::String)
                .str()
                .to_lowercase()
                .str()
                .contains_literal(lit(needle.clone()))
                .fill_null(lit(false))
        })
        .reduce(|a, b| a.or(b))
}

/// One window of the dataset, in view order (search + sort applied), each row
/// tagged with its physical record index.
pub(crate) fn query(
    cache: &DatasetCache,
    root: &str,
    path: &str,
    req: &DatasetQueryReq,
) -> Result<DatasetPage, String> {
    let file = resolve(root, path)?;
    let format = Format::from_path(&file)
        .ok_or_else(|| format!("{path} is not a supported dataset (csv, tsv, jsonl, parquet)"))?;
    let meta = fs::metadata(&file).map_err(|e| format!("could not open {path}: {e}"))?;
    let mtime = meta.modified().map_err(|e| e.to_string())?;
    let started = Instant::now();

    let source = match cache.frame(&file, format, meta.len(), mtime)? {
        Some(df) => df.lazy(),
        None => lazy_source(&file, format)?,
    };

    let mut with_ids = source.with_row_index(ROW_ID, None);
    let schema = with_ids
        .collect_schema()
        .map_err(|e| format!("could not read the schema of {path}: {e}"))?;
    let columns: Vec<ColumnMeta> = schema
        .iter()
        .filter(|(name, _)| name.as_str() != ROW_ID)
        .map(|(name, dt)| ColumnMeta {
            name: name.to_string(),
            dtype: dt.to_string(),
            kind: dtype_kind(dt),
        })
        .collect();

    let mut view = with_ids;
    let needle = req
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    if !needle.is_empty() {
        if let Some(expr) = search_expr(&schema, needle) {
            view = view.filter(expr);
        }
    }

    let total_rows = cache.total_rows(&file, mtime, needle, &view, path)?;

    if let Some(sort_col) = req.sort_by.as_deref() {
        if !schema.contains(sort_col) {
            return Err(format!("no column named {sort_col} in {path}"));
        }
        view = view.sort(
            [sort_col],
            SortMultipleOptions::default()
                .with_order_descending(req.descending)
                .with_nulls_last(true)
                .with_maintain_order(true),
        );
    }

    let window = view
        .slice(req.offset as i64, req.limit.min(MAX_PAGE_ROWS) as IdxSize)
        .collect()
        .map_err(|e| format!("could not read {path}: {e}"))?;

    let height = window.height();
    let row_ids: Vec<u64> = window
        .column(ROW_ID)
        .map_err(|e| e.to_string())?
        .as_materialized_series()
        .iter()
        .map(|av| av.extract::<u64>().unwrap_or(0))
        .collect();
    let mut rows: Vec<Vec<Json>> = (0..height)
        .map(|_| Vec::with_capacity(columns.len()))
        .collect();
    for meta_col in &columns {
        let series = window
            .column(&meta_col.name)
            .map_err(|e| e.to_string())?
            .as_materialized_series();
        for (i, av) in series.iter().enumerate() {
            rows[i].push(json_value(av));
        }
    }

    let editable = format != Format::Parquet || meta.len() <= PARQUET_MAX_EDIT_BYTES;
    Ok(DatasetPage {
        columns,
        rows,
        row_ids,
        total_rows,
        file_size: meta.len(),
        format: format.name(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        editable,
        mtime_ms: file_mtime_ms(&meta)?,
    })
}

// ---- editing --------------------------------------------------------------------

/// The textual form a JSON value takes in a delimited file.
fn csv_text(value: &Json) -> String {
    match value {
        Json::Null => String::new(),
        Json::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The scratch sibling an edit writes before atomically renaming into place.
fn edit_tmp_path(file: &Path) -> PathBuf {
    file.with_file_name(format!(
        ".{}.{}.oxh-edit",
        file.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ))
}

/// Replace [start, end) of `file` with `patch`, streaming everything else
/// byte-for-byte through a temp file that fsyncs and atomically renames into
/// place — a crash mid-edit leaves the original untouched.
fn splice_file(file: &Path, start: u64, end: u64, patch: &[u8]) -> Result<(), String> {
    let err = |e: io::Error| format!("could not rewrite {}: {e}", file.display());
    let mut src = fs::File::open(file).map_err(err)?;
    let perms = src.metadata().map_err(err)?.permissions();
    let tmp_path = edit_tmp_path(file);
    let mut tmp = fs::File::create(&tmp_path).map_err(err)?;
    io::copy(&mut (&mut src).take(start), &mut tmp).map_err(err)?;
    tmp.write_all(patch).map_err(err)?;
    src.seek(SeekFrom::Start(end)).map_err(err)?;
    io::copy(&mut src, &mut tmp).map_err(err)?;
    tmp.sync_all().map_err(err)?;
    drop(tmp);
    fs::set_permissions(&tmp_path, perms).map_err(err)?;
    fs::rename(&tmp_path, file).map_err(err)
}

/// The byte at `off`, or `None` past the end of the file.
fn byte_at(file: &Path, off: u64) -> Result<Option<u8>, String> {
    let err = |e: io::Error| format!("could not read {}: {e}", file.display());
    let mut src = fs::File::open(file).map_err(err)?;
    src.seek(SeekFrom::Start(off)).map_err(err)?;
    let mut byte = [0u8; 1];
    let read = src.read(&mut byte).map_err(err)?;
    Ok((read == 1).then_some(byte[0]))
}

/// Line terminator used by the byte range [start, end) — preserved verbatim
/// so an edit never converts the file's line endings.
fn terminator_of(file: &Path, start: u64, end: u64) -> Result<&'static [u8], String> {
    let err = |e: io::Error| format!("could not read {}: {e}", file.display());
    let mut src = fs::File::open(file).map_err(err)?;
    let tail_len = (end - start).min(2);
    src.seek(SeekFrom::Start(end - tail_len)).map_err(err)?;
    let mut tail = [0u8; 2];
    let read = src.read(&mut tail[..tail_len as usize]).map_err(err)?;
    let tail = &tail[..read];
    Ok(if tail.ends_with(b"\r\n") {
        b"\r\n"
    } else if tail.ends_with(b"\n") {
        b"\n"
    } else {
        b""
    })
}

/// Set one cell of a CSV/TSV file by rewriting only that record. The csv
/// crate parses properly (quoted fields, embedded newlines), and every byte
/// outside the touched record is copied verbatim.
fn edit_delimited(
    file: &Path,
    delimiter: u8,
    row: u64,
    column: &str,
    value: &Json,
) -> Result<(), String> {
    let err = |e: csv::Error| format!("could not parse {}: {e}", file.display());
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_path(file)
        .map_err(err)?;

    let mut record = csv::ByteRecord::new();
    if !rdr.read_byte_record(&mut record).map_err(err)? {
        return Err(format!("{} is empty", file.display()));
    }
    let header_len = record.len();
    let col_idx = record
        .iter()
        .position(|f| f == column.as_bytes())
        .ok_or_else(|| format!("no column named {column} in {}", file.display()))?;

    let mut data_row: u64 = 0;
    loop {
        if !rdr.read_byte_record(&mut record).map_err(err)? {
            return Err(format!("row {row} is past the end of {}", file.display()));
        }
        if data_row == row {
            break;
        }
        data_row += 1;
    }
    let mut start = record
        .position()
        .ok_or_else(|| "record has no file position".to_string())?
        .byte();
    let mut end = rdr.position().byte();
    // With CRLF endings the csv reader's positions can sit between the `\r`
    // and the `\n`: the previous record's residual `\n` lands at our start,
    // and our own trailing `\n` lands past our end. Normalize both so the
    // spliced region is exactly this record plus its full terminator — but
    // only when the `\n` really is the second half of a `\r\n`, so a blank
    // line next to the record is never swallowed.
    if start > 0 && byte_at(file, start)? == Some(b'\n') && byte_at(file, start - 1)? == Some(b'\r')
    {
        start += 1;
    }
    if end > 0 && byte_at(file, end)? == Some(b'\n') && byte_at(file, end - 1)? == Some(b'\r') {
        end += 1;
    }

    let mut fields: Vec<Vec<u8>> = record.iter().map(<[u8]>::to_vec).collect();
    // A short row (flexible parsing) still gets its cell set: pad to the
    // header's width so the field exists.
    while fields.len() < header_len.max(col_idx + 1) {
        fields.push(Vec::new());
    }
    fields[col_idx] = csv_text(value).into_bytes();

    let mut wtr = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_writer(Vec::new());
    wtr.write_record(&fields).map_err(err)?;
    let mut patch = wtr
        .into_inner()
        .map_err(|e| format!("could not serialize the edited row: {e}"))?;
    if patch.last() == Some(&b'\n') {
        patch.pop();
    }
    patch.extend_from_slice(terminator_of(file, start, end)?);
    splice_file(file, start, end, &patch)
}

/// Set one key of a JSONL record by rewriting only that line; every other
/// line is copied byte-for-byte (serde_json preserves key order).
fn edit_jsonl(file: &Path, row: u64, column: &str, value: &Json) -> Result<(), String> {
    let err = |e: io::Error| format!("could not read {}: {e}", file.display());
    let mut reader = BufReader::new(fs::File::open(file).map_err(err)?);
    let mut line: Vec<u8> = Vec::new();
    let mut offset: u64 = 0;
    let mut data_row: u64 = 0;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line).map_err(err)? as u64;
        if read == 0 {
            return Err(format!("row {row} is past the end of {}", file.display()));
        }
        let is_record = !line.iter().all(|b| b.is_ascii_whitespace());
        if is_record && data_row == row {
            break;
        }
        if is_record {
            data_row += 1;
        }
        offset += read;
    }
    let start = offset;
    let end = offset + line.len() as u64;

    let text = std::str::from_utf8(&line).map_err(|_| format!("row {row} is not valid UTF-8"))?;
    let mut parsed: serde_json::Map<String, Json> = serde_json::from_str(text.trim_end())
        .map_err(|e| format!("row {row} is not a JSON object: {e}"))?;
    parsed.insert(column.to_string(), value.clone());
    let mut patch = serde_json::to_string(&parsed)
        .map_err(|e| e.to_string())?
        .into_bytes();
    patch.extend_from_slice(terminator_of(file, start, end)?);
    splice_file(file, start, end, &patch)
}

/// Set one cell of a Parquet file. Columnar files have no record to splice,
/// so this reads the frame, patches the cell (cast to the column's dtype),
/// and rewrites the file — which is why it's capped by size.
fn edit_parquet(file: &Path, row: u64, column: &str, value: &Json) -> Result<(), String> {
    let size = fs::metadata(file).map_err(|e| e.to_string())?.len();
    if size > PARQUET_MAX_EDIT_BYTES {
        return Err(format!(
            "{} is too large to edit ({} MB; the cap is {} MB — parquet edits rewrite the file)",
            file.display(),
            size / (1024 * 1024),
            PARQUET_MAX_EDIT_BYTES / (1024 * 1024)
        ));
    }
    let lazy = lazy_source(file, Format::Parquet)?;
    let schema = lazy
        .clone()
        .collect_schema()
        .map_err(|e| format!("could not read the schema of {}: {e}", file.display()))?;
    let dtype = schema
        .get(column)
        .ok_or_else(|| format!("no column named {column} in {}", file.display()))?
        .clone();
    if matches!(
        dtype,
        DataType::List(_) | DataType::Array(_, _) | DataType::Struct(_)
    ) {
        return Err(format!(
            "{column} holds nested values, which can't be edited as a cell"
        ));
    }
    let patched = match value {
        Json::Null => lit(Null {}),
        Json::Bool(b) => lit(*b),
        Json::Number(n) if n.is_i64() => lit(n.as_i64().unwrap_or_default()),
        Json::Number(n) => lit(n.as_f64().unwrap_or_default()),
        Json::String(s) => lit(s.clone()),
        other => lit(other.to_string()),
    }
    .cast(dtype);

    let mut df = lazy
        .with_row_index(ROW_ID, None)
        .with_column(
            when(col(ROW_ID).eq(lit(row)))
                .then(patched)
                .otherwise(col(column))
                .alias(column),
        )
        .collect()
        .map_err(|e| format!("could not apply the edit: {e}"))?;
    let _ = df.drop_in_place(ROW_ID).map_err(|e| e.to_string())?;

    let err = |e: io::Error| format!("could not rewrite {}: {e}", file.display());
    let perms = fs::metadata(file).map_err(err)?.permissions();
    let tmp_path = edit_tmp_path(file);
    let tmp = fs::File::create(&tmp_path).map_err(err)?;
    ParquetWriter::new(tmp)
        .finish(&mut df)
        .map_err(|e| format!("could not write {}: {e}", file.display()))?;
    fs::File::open(&tmp_path)
        .map_err(err)?
        .sync_all()
        .map_err(err)?;
    fs::set_permissions(&tmp_path, perms).map_err(err)?;
    fs::rename(&tmp_path, file).map_err(err)
}

/// Set one cell, addressed by physical record index + column name. When the
/// caller supplies the mtime its page was read at, an edit against a file
/// that changed underneath is refused instead of landing on the wrong record.
/// Returns the file's new mtime so the caller can keep editing.
pub(crate) fn write_cell(
    cache: &DatasetCache,
    root: &str,
    path: &str,
    row: u64,
    column: &str,
    value: &Json,
    expected_mtime_ms: Option<u64>,
) -> Result<u64, String> {
    let file = resolve(root, path)?;
    let format = Format::from_path(&file)
        .ok_or_else(|| format!("{path} is not a supported dataset (csv, tsv, jsonl, parquet)"))?;
    let _serialized = cache.edit_lock.lock().unwrap();
    if let Some(expected) = expected_mtime_ms {
        let meta = fs::metadata(&file).map_err(|e| format!("could not open {path}: {e}"))?;
        if file_mtime_ms(&meta)? != expected {
            return Err(format!(
                "{path} changed on disk since it was loaded — refresh and retry"
            ));
        }
    }
    let result = match format {
        Format::Csv | Format::Tsv => edit_delimited(&file, format.delimiter(), row, column, value),
        Format::Jsonl => edit_jsonl(&file, row, column, value),
        Format::Parquet => edit_parquet(&file, row, column, value),
    };
    cache.invalidate(&file);
    result?;
    let meta = fs::metadata(&file).map_err(|e| format!("could not open {path}: {e}"))?;
    file_mtime_ms(&meta)
}

// ---- the Tauri commands ----------------------------------------------------------

/// Read one window of a dataset for the grid. Heavy parsing runs on a
/// blocking thread so scrolling never stalls the UI event loop.
#[tauri::command]
pub(crate) async fn dataset_query(
    state: tauri::State<'_, DatasetCache>,
    root: String,
    path: String,
    req: DatasetQueryReq,
) -> Result<DatasetPage, String> {
    let cache = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || query(&cache, &root, &path, &req))
        .await
        .map_err(|e| e.to_string())?
}

/// Write one edited cell back to the file on disk. Returns the file's new
/// mtime (ms) — the staleness token for the next edit.
#[tauri::command]
pub(crate) async fn dataset_write_cell(
    state: tauri::State<'_, DatasetCache>,
    root: String,
    path: String,
    row: u64,
    column: String,
    value: Json,
    expected_mtime_ms: Option<u64>,
) -> Result<u64, String> {
    let cache = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        write_cell(
            &cache,
            &root,
            &path,
            row,
            &column,
            &value,
            expected_mtime_ms,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oxen-harness-dataset-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn req(offset: u64, limit: u64) -> DatasetQueryReq {
        DatasetQueryReq {
            offset,
            limit,
            sort_by: None,
            descending: false,
            search: None,
        }
    }

    fn cell(page: &DatasetPage, row: usize, col: &str) -> Json {
        let idx = page.columns.iter().position(|c| c.name == col).unwrap();
        page.rows[row][idx].clone()
    }

    #[test]
    fn pages_sorts_and_searches_a_csv() {
        let dir = workspace("csv-query");
        let root = dir.display().to_string();
        let mut body = String::from("name,score,city\n");
        for i in 0..500 {
            body.push_str(&format!("row{i},{},city{}\n", i * 2, i % 10));
        }
        fs::write(dir.join("data.csv"), body).unwrap();
        let cache = DatasetCache::default();

        let page = query(&cache, &root, "data.csv", &req(0, 50)).unwrap();
        assert_eq!(page.total_rows, 500);
        assert_eq!(page.rows.len(), 50);
        assert_eq!(
            page.columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["name", "score", "city"]
        );
        assert_eq!(page.columns[1].kind, "int");
        assert_eq!(cell(&page, 0, "name"), json!("row0"));
        assert_eq!(page.row_ids[0], 0);

        // A deep window, sorted descending: highest score first, and each row
        // still knows its physical index.
        let sorted = query(
            &cache,
            &root,
            "data.csv",
            &DatasetQueryReq {
                offset: 0,
                limit: 3,
                sort_by: Some("score".into()),
                descending: true,
                search: None,
            },
        )
        .unwrap();
        assert_eq!(cell(&sorted, 0, "score"), json!(998));
        assert_eq!(sorted.row_ids[0], 499);

        // Search narrows the view (and the reported total) across columns.
        let found = query(
            &cache,
            &root,
            "data.csv",
            &DatasetQueryReq {
                offset: 0,
                limit: 50,
                sort_by: None,
                descending: false,
                search: Some("ROW49,".into()),
            },
        )
        .unwrap();
        assert_eq!(found.total_rows, 0); // search matches cells, not raw lines
        let found = query(
            &cache,
            &root,
            "data.csv",
            &DatasetQueryReq {
                offset: 0,
                limit: 50,
                sort_by: None,
                descending: false,
                search: Some("ROW499".into()),
            },
        )
        .unwrap();
        assert_eq!(found.total_rows, 1);
        assert_eq!(found.row_ids[0], 499);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn csv_edit_touches_only_the_edited_record() {
        let dir = workspace("csv-edit");
        let root = dir.display().to_string();
        // Quoted commas and CRLF endings both survive an edit untouched.
        let body = "name,note\r\nalice,\"hello, world\"\r\nbob,plain\r\ncarol,\"multi\nline\"\r\n";
        fs::write(dir.join("notes.csv"), body).unwrap();
        let cache = DatasetCache::default();

        write_cell(
            &cache,
            &root,
            "notes.csv",
            1,
            "note",
            &json!("edited"),
            None,
        )
        .unwrap();
        let after = fs::read_to_string(dir.join("notes.csv")).unwrap();
        assert_eq!(
            after,
            "name,note\r\nalice,\"hello, world\"\r\nbob,edited\r\ncarol,\"multi\nline\"\r\n"
        );

        // The grid sees the new value.
        let page = query(&cache, &root, "notes.csv", &req(0, 10)).unwrap();
        assert_eq!(cell(&page, 1, "note"), json!("edited"));

        // Unknown columns and rows fail loudly instead of corrupting the file.
        assert!(write_cell(&cache, &root, "notes.csv", 0, "missing", &json!("x"), None).is_err());
        assert!(write_cell(&cache, &root, "notes.csv", 99, "note", &json!("x"), None).is_err());
        assert!(query(&cache, &root, "../notes.csv", &req(0, 1)).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn jsonl_round_trips_ragged_records_and_edits_one_line() {
        let dir = workspace("jsonl");
        let root = dir.display().to_string();
        let body = r#"{"id": 1, "tag": "a", "extra": true}
{"id": 2, "tag": "b"}
{"id": 3}
"#;
        fs::write(dir.join("events.jsonl"), body).unwrap();
        let cache = DatasetCache::default();

        let page = query(&cache, &root, "events.jsonl", &req(0, 10)).unwrap();
        assert_eq!(page.total_rows, 3);
        assert_eq!(cell(&page, 2, "tag"), Json::Null);
        assert_eq!(page.format, "jsonl");

        write_cell(
            &cache,
            &root,
            "events.jsonl",
            1,
            "tag",
            &json!("edited"),
            None,
        )
        .unwrap();
        let after = fs::read_to_string(dir.join("events.jsonl")).unwrap();
        let lines: Vec<&str> = after.lines().collect();
        assert_eq!(lines[0], r#"{"id": 1, "tag": "a", "extra": true}"#); // untouched, byte-identical
        assert_eq!(lines[1], r#"{"id":2,"tag":"edited"}"#);
        assert_eq!(lines[2], r#"{"id": 3}"#);

        // Numbers stay numbers through an edit.
        write_cell(&cache, &root, "events.jsonl", 2, "id", &json!(42), None).unwrap();
        let page = query(&cache, &root, "events.jsonl", &req(0, 10)).unwrap();
        assert_eq!(cell(&page, 2, "id"), json!(42));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parquet_queries_and_edits_with_dtype_casts() {
        let dir = workspace("parquet");
        let root = dir.display().to_string();
        let mut df = df! {
            "id" => [1i64, 2, 3, 4],
            "name" => ["ada", "grace", "alan", "edsger"],
            "score" => [0.5f64, 0.9, 0.7, 0.8],
        }
        .unwrap();
        let file = fs::File::create(dir.join("people.parquet")).unwrap();
        ParquetWriter::new(file).finish(&mut df).unwrap();
        let cache = DatasetCache::default();

        let page = query(&cache, &root, "people.parquet", &req(0, 10)).unwrap();
        assert_eq!(page.total_rows, 4);
        assert_eq!(page.columns[2].kind, "float");
        assert!(page.editable);

        let sorted = query(
            &cache,
            &root,
            "people.parquet",
            &DatasetQueryReq {
                offset: 0,
                limit: 2,
                sort_by: Some("name".into()),
                descending: false,
                search: None,
            },
        )
        .unwrap();
        assert_eq!(cell(&sorted, 0, "name"), json!("ada"));
        assert_eq!(sorted.row_ids[0], 0);
        assert_eq!(cell(&sorted, 1, "name"), json!("alan"));
        assert_eq!(sorted.row_ids[1], 2);

        // An edit casts to the column dtype (JSON 1 -> i64) and persists.
        write_cell(&cache, &root, "people.parquet", 2, "score", &json!(1), None).unwrap();
        let page = query(&cache, &root, "people.parquet", &req(0, 10)).unwrap();
        assert_eq!(cell(&page, 2, "score"), json!(1.0));
        assert_eq!(cell(&page, 2, "name"), json!("alan"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn search_never_matches_the_hidden_row_index() {
        let dir = workspace("search-rowid");
        let root = dir.display().to_string();
        // 100 alphabetic-only rows: a digit search can only match the injected
        // row-id column, and that column must not count as data.
        let mut body = String::from("word\n");
        for _ in 0..100 {
            body.push_str("alpha\n");
        }
        fs::write(dir.join("words.csv"), body).unwrap();
        let cache = DatasetCache::default();
        let found = query(
            &cache,
            &root,
            "words.csv",
            &DatasetQueryReq {
                offset: 0,
                limit: 10,
                sort_by: None,
                descending: false,
                search: Some("5".into()),
            },
        )
        .unwrap();
        assert_eq!(found.total_rows, 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn csv_edit_keeps_blank_lines_next_to_the_record() {
        let dir = workspace("csv-blank");
        let root = dir.display().to_string();
        fs::write(dir.join("gaps.csv"), "a,b\n1,x\n\n2,y\n").unwrap();
        let cache = DatasetCache::default();
        write_cell(&cache, &root, "gaps.csv", 0, "b", &json!("edited"), None).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("gaps.csv")).unwrap(),
            "a,b\n1,edited\n\n2,y\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn jsonl_edit_preserves_non_alphabetical_key_order() {
        let dir = workspace("jsonl-order");
        let root = dir.display().to_string();
        fs::write(dir.join("o.jsonl"), "{\"tag\":\"a\",\"id\":1}\n").unwrap();
        let cache = DatasetCache::default();
        write_cell(&cache, &root, "o.jsonl", 0, "id", &json!(2), None).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("o.jsonl")).unwrap(),
            "{\"tag\":\"a\",\"id\":2}\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_edits_are_refused_and_fresh_ones_return_the_new_token() {
        let dir = workspace("mtime-guard");
        let root = dir.display().to_string();
        fs::write(dir.join("g.csv"), "a,b\n1,x\n").unwrap();
        let cache = DatasetCache::default();
        let page = query(&cache, &root, "g.csv", &req(0, 10)).unwrap();

        // A stale token (the file "changed" since that fetch) is refused…
        let err = write_cell(
            &cache,
            &root,
            "g.csv",
            0,
            "b",
            &json!("y"),
            Some(page.mtime_ms - 1),
        )
        .unwrap_err();
        assert!(err.contains("changed on disk"), "{err}");
        assert_eq!(fs::read_to_string(dir.join("g.csv")).unwrap(), "a,b\n1,x\n");

        // …the current token lands, and returns the next token for chaining.
        let next = write_cell(
            &cache,
            &root,
            "g.csv",
            0,
            "b",
            &json!("y"),
            Some(page.mtime_ms),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(dir.join("g.csv")).unwrap(), "a,b\n1,y\n");
        write_cell(&cache, &root, "g.csv", 0, "a", &json!(9), Some(next)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("g.csv")).unwrap(), "a,b\n9,y\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unsupported_extensions_are_refused() {
        let dir = workspace("unsupported");
        let root = dir.display().to_string();
        fs::write(dir.join("data.txt"), "a,b\n1,2\n").unwrap();
        let cache = DatasetCache::default();
        assert!(query(&cache, &root, "data.txt", &req(0, 10)).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
