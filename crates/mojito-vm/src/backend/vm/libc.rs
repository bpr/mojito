//! The VM's execution of the closed libc callee table behind
//! `external_call` (`mojito_types::ffi::CALLEES`).
//!
//! Every callee reproduces the observable contract of its glibc namesake —
//! return value, `errno`, and the bytes written through pointer arguments —
//! on top of Rust's standard library, so the VM stays a pure-Rust oracle
//! and the native backend's real C calls produce identical program output.
//! Host state lives in [`HostState`]: a descriptor table (fds 0-2 are the
//! process streams, with fd 1 appending to the captured stdout buffer so
//! `write(1, ..)` and `print` interleave exactly), open directory
//! snapshots, the `errno` slot, and a per-VM environment overlay.

use super::*;
use crate::runtime::{SimdLanes, type_name};
use mojito_ast::ast::Dtype;
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, DirEntryExt, FileTypeExt, MetadataExt, OpenOptionsExt};

/// Host-facing state of one VM: the libc table's descriptors, directory
/// streams, `errno`, and environment overlay.
#[derive(Default)]
pub(super) struct HostState {
    /// Open files by descriptor number (3 and up; 0-2 are the process
    /// streams and never appear here).
    files: HashMap<i32, std::fs::File>,
    /// Open directory streams keyed by the identity allocation `opendir`
    /// returned.
    dirs: HashMap<u64, DirState>,
    /// The one-slot `Int32` allocation `__errno_location` points at, created
    /// on first use.
    errno: Option<u64>,
    /// `setenv`/`unsetenv` writes. The process environment is global and
    /// the test binaries run many programs concurrently, so a program's
    /// writes stay private to its VM; `getenv` reads the overlay first.
    env: HashMap<Vec<u8>, Option<Vec<u8>>>,
    /// `strerror` texts already materialized as C strings, by code.
    strerror_cache: HashMap<i32, u64>,
}

/// One open directory stream: the entries still to return and the 280-byte
/// `struct dirent` image allocation `readdir` rewrites for each of them.
struct DirState {
    pending: VecDeque<DirentImage>,
    buffer: u64,
}

struct DirentImage {
    ino: u64,
    kind: u8,
    name: Vec<u8>,
}

const EIO: i32 = 5;
const EBADF: i32 = 9;
const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
const ESPIPE: i32 = 29;
const ERANGE: i32 = 34;

const O_ACCMODE: i64 = 0o3;
const O_CREAT: i64 = 0o100;
const O_EXCL: i64 = 0o200;
const O_TRUNC: i64 = 0o1000;
const O_APPEND: i64 = 0o2000;

const DT_UNKNOWN: u8 = 0;
const DT_FIFO: u8 = 1;
const DT_CHR: u8 = 2;
const DT_DIR: u8 = 4;
const DT_BLK: u8 = 6;
const DT_REG: u8 = 8;
const DT_LNK: u8 = 10;
const DT_SOCK: u8 = 12;

/// The glibc `struct dirent` layout the image reproduces: `d_name` starts
/// at byte 19 and holds at most 255 bytes plus the terminator.
const DIRENT_NAME_OFFSET: usize = 19;
const DIRENT_SIZE: i64 = 280;

/// A frame id no live frame carries: reference handles resolved under it
/// always go through the pushed caller mirror.
const NO_FRAME: FrameId = FrameId(u64::MAX);

impl VmBackend {
    /// Execute one allowlisted libc callee. `arg_types` are the checked
    /// argument types (a pointer argument's element type selects the lane
    /// dtype of bytes written through it).
    pub(super) fn external_call(
        &mut self,
        callee: &str,
        args: Vec<Value>,
        arg_types: &[Option<Ty>],
    ) -> Result<Value, RuntimeError> {
        // Every buffer byte a callee writes is stored as a `UInt8` lane
        // whatever the pointer's element type (`c_char` buffers included), so
        // `Byte` and `c_char` views of one buffer agree on every lane value.
        let _ = arg_types;
        match callee {
            "open" => {
                let path = self.c_string_arg(callee, &args, 0)?;
                let flags = int_arg(callee, &args, 1)?;
                let mode = args.get(2).map(|v| c_integer(callee, 2, v)).transpose()?;
                self.libc_open(&path, flags, mode.unwrap_or(0o666))
            }
            "read" => {
                let fd = int_arg(callee, &args, 0)? as i32;
                let buffer = pointer_arg(callee, &args, 1)?;
                let count = int_arg(callee, &args, 2)?;
                self.libc_read(fd, buffer, count)
            }
            "write" => {
                let fd = int_arg(callee, &args, 0)? as i32;
                let buffer = pointer_arg(callee, &args, 1)?;
                let count = int_arg(callee, &args, 2)?;
                self.libc_write(fd, buffer, count)
            }
            "close" => {
                let fd = int_arg(callee, &args, 0)? as i32;
                if (0..=2).contains(&fd) || self.host.files.remove(&fd).is_some() {
                    Ok(Value::Int(0))
                } else {
                    self.fail_code(EBADF)
                }
            }
            "lseek" => {
                let fd = int_arg(callee, &args, 0)? as i32;
                let offset = int_arg(callee, &args, 1)?;
                let whence = int_arg(callee, &args, 2)?;
                self.libc_lseek(fd, offset, whence)
            }
            "unlink" => {
                let path = self.c_string_arg(callee, &args, 0)?;
                self.status(std::fs::remove_file(os_path(&path)))
            }
            "rmdir" => {
                let path = self.c_string_arg(callee, &args, 0)?;
                self.status(std::fs::remove_dir(os_path(&path)))
            }
            "mkdir" => {
                let path = self.c_string_arg(callee, &args, 0)?;
                let mode = int_arg(callee, &args, 1)?;
                let result = std::fs::DirBuilder::new()
                    .mode(mode as u32)
                    .create(os_path(&path));
                self.status(result)
            }
            "opendir" => {
                let path = self.c_string_arg(callee, &args, 0)?;
                self.libc_opendir(&path)
            }
            "readdir" => {
                let handle = pointer_arg(callee, &args, 0)?;
                self.libc_readdir(handle)
            }
            "closedir" => {
                let handle = pointer_arg(callee, &args, 0)?;
                match handle {
                    Some(Value::Pointer { allocation, .. })
                        if self.host.dirs.remove(&allocation).is_some() =>
                    {
                        Ok(Value::Int(0))
                    }
                    _ => self.fail_code(EBADF),
                }
            }
            "getcwd" => {
                let buffer = pointer_arg(callee, &args, 0)?;
                let size = int_arg(callee, &args, 1)?;
                self.libc_getcwd(buffer, size)
            }
            "getenv" => {
                let name = self.c_string_arg(callee, &args, 0)?;
                match self.env_lookup(&name) {
                    Some(value) => self.alloc_c_string(&value),
                    None => Ok(null_pointer()),
                }
            }
            "setenv" => {
                let name = self.c_string_arg(callee, &args, 0)?;
                let value = self.c_string_arg(callee, &args, 1)?;
                let overwrite = int_arg(callee, &args, 2)? != 0;
                if name.is_empty() || name.contains(&b'=') {
                    return self.fail_code(EINVAL);
                }
                if overwrite || self.env_lookup(&name).is_none() {
                    self.host.env.insert(name, Some(value));
                }
                Ok(Value::Int(0))
            }
            "unsetenv" => {
                let name = self.c_string_arg(callee, &args, 0)?;
                if name.is_empty() || name.contains(&b'=') {
                    return self.fail_code(EINVAL);
                }
                self.host.env.insert(name, None);
                Ok(Value::Int(0))
            }
            "strerror" => {
                let code = int_arg(callee, &args, 0)? as i32;
                if let Some(allocation) = self.host.strerror_cache.get(&code) {
                    return Ok(Value::Pointer {
                        allocation: *allocation,
                        offset: 0,
                    });
                }
                let pointer = self.alloc_c_string(strerror_text(code).as_bytes())?;
                if let Value::Pointer { allocation, .. } = pointer {
                    self.host.strerror_cache.insert(code, allocation);
                }
                Ok(pointer)
            }
            "__errno_location" => self.errno_pointer(),
            "memcpy" => {
                let destination = pointer_arg(callee, &args, 0)?;
                let source = pointer_arg(callee, &args, 1)?;
                let count = int_arg(callee, &args, 2)?;
                let (Some(destination), Some(source)) = (destination, source) else {
                    return Err(RuntimeError::TypeError(
                        "vm: memcpy through a null pointer".to_string(),
                    ));
                };
                if count > 0 {
                    let bytes = self.read_bytes(&source, count as usize)?;
                    self.write_bytes(&destination, &bytes)?;
                }
                Ok(destination)
            }
            "strlen" => {
                let text = self.c_string_arg(callee, &args, 0)?;
                Ok(Value::Int(text.len() as i64))
            }
            "__xstat" | "__lxstat" => {
                let path = self.c_string_arg(callee, &args, 1)?;
                let handle = pointer_arg(callee, &args, 2)?;
                let metadata = if callee == "__xstat" {
                    std::fs::metadata(os_path(&path))
                } else {
                    std::fs::symlink_metadata(os_path(&path))
                };
                match metadata {
                    Ok(metadata) => {
                        let Some(handle) = handle else {
                            return self.fail_code(EFAULT);
                        };
                        self.fill_stat(&handle, &metadata)?;
                        Ok(Value::Int(0))
                    }
                    Err(error) => self.fail(&error),
                }
            }
            other => Err(RuntimeError::Unsupported(format!(
                "vm: external_call callee '{other}' is not in the libc allowlist"
            ))),
        }
    }

    fn libc_open(&mut self, path: &[u8], flags: i64, mode: i64) -> Result<Value, RuntimeError> {
        let access = flags & O_ACCMODE;
        let mut options = std::fs::OpenOptions::new();
        options
            .read(access == 0 || access == 2)
            .write(access == 1 || access == 2)
            .append(flags & O_APPEND != 0)
            .truncate(flags & O_TRUNC != 0)
            .mode(mode as u32)
            .custom_flags((flags & !(O_ACCMODE | O_CREAT | O_EXCL | O_TRUNC | O_APPEND)) as i32);
        if flags & O_CREAT != 0 {
            if flags & O_EXCL != 0 {
                options.create_new(true);
            } else {
                options.create(true);
            }
        }
        match options.open(os_path(path)) {
            Ok(file) => {
                let fd = (3..i32::MAX)
                    .find(|fd| !self.host.files.contains_key(fd))
                    .expect("descriptor space");
                self.host.files.insert(fd, file);
                Ok(Value::Int(i64::from(fd)))
            }
            Err(error) => self.fail(&error),
        }
    }

    fn libc_read(
        &mut self,
        fd: i32,
        buffer: Option<Value>,
        count: i64,
    ) -> Result<Value, RuntimeError> {
        if count < 0 {
            return self.fail_code(EINVAL);
        }
        if count == 0 {
            return Ok(Value::Int(0));
        }
        let Some(buffer) = buffer else {
            return self.fail_code(EFAULT);
        };
        let mut bytes = vec![0u8; count as usize];
        let read = match fd {
            0 => match self.input_override.as_mut() {
                Some(cursor) => cursor.read(&mut bytes),
                None => std::io::stdin().lock().read(&mut bytes),
            },
            1 | 2 => return self.fail_code(EBADF),
            _ => match self.host.files.get_mut(&fd) {
                Some(file) => file.read(&mut bytes),
                None => return self.fail_code(EBADF),
            },
        };
        match read {
            Ok(n) => {
                self.write_bytes(&buffer, &bytes[..n])?;
                Ok(Value::Int(n as i64))
            }
            Err(error) => self.fail(&error),
        }
    }

    fn libc_write(
        &mut self,
        fd: i32,
        buffer: Option<Value>,
        count: i64,
    ) -> Result<Value, RuntimeError> {
        if count < 0 {
            return self.fail_code(EINVAL);
        }
        if count == 0 {
            return Ok(Value::Int(0));
        }
        let Some(buffer) = buffer else {
            return self.fail_code(EFAULT);
        };
        let bytes = self.read_bytes(&buffer, count as usize)?;
        let written = match fd {
            0 => return self.fail_code(EBADF),
            1 => {
                self.output.push_str(&String::from_utf8_lossy(&bytes));
                Ok(bytes.len())
            }
            2 => std::io::stderr().write_all(&bytes).map(|()| bytes.len()),
            _ => match self.host.files.get_mut(&fd) {
                Some(file) => file.write(&bytes),
                None => return self.fail_code(EBADF),
            },
        };
        match written {
            Ok(n) => Ok(Value::Int(n as i64)),
            Err(error) => self.fail(&error),
        }
    }

    fn libc_lseek(&mut self, fd: i32, offset: i64, whence: i64) -> Result<Value, RuntimeError> {
        if (0..=2).contains(&fd) {
            return self.fail_code(ESPIPE);
        }
        let position = match whence {
            0 if offset >= 0 => SeekFrom::Start(offset as u64),
            1 => SeekFrom::Current(offset),
            2 => SeekFrom::End(offset),
            _ => return self.fail_code(EINVAL),
        };
        let Some(file) = self.host.files.get_mut(&fd) else {
            return self.fail_code(EBADF);
        };
        match file.seek(position) {
            Ok(position) => Ok(Value::Int(position as i64)),
            Err(error) => self.fail(&error),
        }
    }

    fn libc_opendir(&mut self, path: &[u8]) -> Result<Value, RuntimeError> {
        let directory = os_path(path);
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) => {
                self.set_errno(error.raw_os_error().unwrap_or(EIO))?;
                return Ok(null_pointer());
            }
        };
        // libc yields `.` and `..` first; upstream's `listdir` filters them
        // by name, so both backends see the same stream.
        let mut pending = VecDeque::new();
        let self_ino = std::fs::metadata(directory).map(|m| m.ino()).unwrap_or(0);
        let parent_ino = std::fs::metadata(directory.join(".."))
            .map(|m| m.ino())
            .unwrap_or(0);
        pending.push_back(DirentImage {
            ino: self_ino,
            kind: DT_DIR,
            name: b".".to_vec(),
        });
        pending.push_back(DirentImage {
            ino: parent_ino,
            kind: DT_DIR,
            name: b"..".to_vec(),
        });
        for entry in entries.flatten() {
            let kind = entry.file_type().map(dirent_kind).unwrap_or(DT_UNKNOWN);
            pending.push_back(DirentImage {
                ino: entry.ino(),
                kind,
                name: entry.file_name().as_bytes().to_vec(),
            });
        }
        let identity = self.heap_alloc(1, 8)?;
        let buffer = self.heap_alloc(DIRENT_SIZE, 8)?;
        let (
            Value::Pointer { allocation, .. },
            Value::Pointer {
                allocation: buffer, ..
            },
        ) = (&identity, buffer)
        else {
            unreachable!("heap_alloc returns pointers");
        };
        self.host
            .dirs
            .insert(*allocation, DirState { pending, buffer });
        Ok(identity)
    }

    fn libc_readdir(&mut self, handle: Option<Value>) -> Result<Value, RuntimeError> {
        let Some(Value::Pointer { allocation, .. }) = handle else {
            self.set_errno(EBADF)?;
            return Ok(null_pointer());
        };
        let Some(state) = self.host.dirs.get_mut(&allocation) else {
            self.set_errno(EBADF)?;
            return Ok(null_pointer());
        };
        let Some(entry) = state.pending.pop_front() else {
            return Ok(null_pointer());
        };
        let buffer = state.buffer;
        let name_len = entry.name.len().min(255);
        let mut image = vec![0u8; DIRENT_NAME_OFFSET + name_len + 1];
        image[..8].copy_from_slice(&entry.ino.to_le_bytes());
        // d_off (bytes 8..16) is opaque and stays 0.
        let reclen = ((DIRENT_NAME_OFFSET + name_len + 1 + 7) & !7) as u16;
        image[16..18].copy_from_slice(&reclen.to_le_bytes());
        image[18] = entry.kind;
        image[DIRENT_NAME_OFFSET..DIRENT_NAME_OFFSET + name_len]
            .copy_from_slice(&entry.name[..name_len]);
        let pointer = Value::Pointer {
            allocation: buffer,
            offset: 0,
        };
        self.write_bytes(&pointer, &image)?;
        Ok(pointer)
    }

    fn libc_getcwd(&mut self, buffer: Option<Value>, size: i64) -> Result<Value, RuntimeError> {
        let Some(buffer) = buffer else {
            self.set_errno(EFAULT)?;
            return Ok(null_pointer());
        };
        let cwd = match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(error) => {
                self.set_errno(error.raw_os_error().unwrap_or(EIO))?;
                return Ok(null_pointer());
            }
        };
        let mut bytes = cwd.as_os_str().as_bytes().to_vec();
        if bytes.len() as i64 + 1 > size {
            self.set_errno(ERANGE)?;
            return Ok(null_pointer());
        }
        bytes.push(0);
        self.write_bytes(&buffer, &bytes)?;
        Ok(buffer)
    }

    /// Fill the caller's `_c_stat` struct (upstream's field names) from
    /// `metadata`, keeping each field's existing scalar representation.
    fn fill_stat(
        &mut self,
        handle: &Value,
        metadata: &std::fs::Metadata,
    ) -> Result<(), RuntimeError> {
        let mut record = self.read_reference(handle, NO_FRAME, &[])?;
        let Value::Struct { fields, .. } = &mut record else {
            return Err(RuntimeError::TypeError(
                "vm: __xstat expects a Pointer to a stat record struct".to_string(),
            ));
        };
        let times = [
            ("st_atimespec", metadata.atime(), metadata.atime_nsec()),
            ("st_mtimespec", metadata.mtime(), metadata.mtime_nsec()),
            ("st_ctimespec", metadata.ctime(), metadata.ctime_nsec()),
        ];
        for (name, value) in fields.iter_mut() {
            let scalar = match name.as_str() {
                "st_dev" => Some(metadata.dev() as i128),
                "st_ino" => Some(metadata.ino() as i128),
                "st_nlink" => Some(metadata.nlink() as i128),
                "st_mode" => Some(i128::from(metadata.mode())),
                "st_uid" => Some(i128::from(metadata.uid())),
                "st_gid" => Some(i128::from(metadata.gid())),
                "st_rdev" => Some(metadata.rdev() as i128),
                "st_size" => Some(i128::from(metadata.size())),
                "st_blksize" => Some(i128::from(metadata.blksize())),
                "st_blocks" => Some(i128::from(metadata.blocks())),
                _ => None,
            };
            if let Some(scalar) = scalar {
                *value = retype_scalar(value, scalar);
                continue;
            }
            if let Some((_, seconds, nanoseconds)) = times.iter().find(|(t, _, _)| t == name)
                && let Value::Struct { fields: spec, .. } = value
            {
                for (spec_name, spec_value) in spec.iter_mut() {
                    match spec_name.as_str() {
                        "tv_sec" => *spec_value = retype_scalar(spec_value, i128::from(*seconds)),
                        "tv_subsec" => {
                            *spec_value = retype_scalar(spec_value, i128::from(*nanoseconds))
                        }
                        _ => {}
                    }
                }
            }
        }
        self.write_reference(handle, NO_FRAME, &mut [], record)
    }

    /// Read a NUL-terminated string argument (a `CStringSlice` view or a byte
    /// `Pointer`).
    fn c_string_arg(
        &self,
        callee: &str,
        args: &[Value],
        index: usize,
    ) -> Result<Vec<u8>, RuntimeError> {
        match pointer_arg(callee, args, index)? {
            Some(pointer) => self.read_c_string(&pointer),
            None => Err(RuntimeError::TypeError(format!(
                "vm: external_call[\"{callee}\"] argument {} is a null C string",
                index + 1
            ))),
        }
    }

    fn read_c_string(&self, pointer: &Value) -> Result<Vec<u8>, RuntimeError> {
        let Value::Pointer { allocation, offset } = pointer else {
            return Err(RuntimeError::TypeError(
                "vm: a C string argument must be a heap byte pointer".to_string(),
            ));
        };
        let mut bytes = Vec::new();
        for index in 0.. {
            let (region, slot) = self.heap_index(*allocation, *offset, index)?;
            match self.heap[region].slots.get(slot) {
                Some(Value::Simd {
                    lanes: SimdLanes::Int(lanes),
                    ..
                }) if lanes.len() == 1 => {
                    let byte = lanes[0] as u8;
                    if byte == 0 {
                        return Ok(bytes);
                    }
                    bytes.push(byte);
                }
                _ => {
                    return Err(RuntimeError::TypeError(
                        "vm: external_call C string is not NUL-terminated inside its allocation"
                            .to_string(),
                    ));
                }
            }
        }
        unreachable!()
    }

    fn read_bytes(&self, pointer: &Value, count: usize) -> Result<Vec<u8>, RuntimeError> {
        let Value::Pointer { allocation, offset } = pointer else {
            return Err(RuntimeError::TypeError(
                "vm: a byte buffer argument must be a heap pointer".to_string(),
            ));
        };
        let mut bytes = Vec::with_capacity(count);
        for index in 0..count as i64 {
            let (region, slot) = self.heap_index(*allocation, *offset, index)?;
            match self.heap[region].slots.get(slot) {
                Some(Value::Simd {
                    lanes: SimdLanes::Int(lanes),
                    ..
                }) if lanes.len() == 1 => bytes.push(lanes[0] as u8),
                other => {
                    return Err(RuntimeError::TypeError(format!(
                        "vm: external_call byte buffer slot is {other:?}, not a byte"
                    )));
                }
            }
        }
        Ok(bytes)
    }

    fn write_bytes(&mut self, pointer: &Value, bytes: &[u8]) -> Result<(), RuntimeError> {
        let Value::Pointer { allocation, offset } = pointer else {
            return Err(RuntimeError::TypeError(
                "vm: a byte buffer argument must be a heap pointer".to_string(),
            ));
        };
        for (index, byte) in bytes.iter().enumerate() {
            let (region, slot) = self.heap_index(*allocation, *offset, index as i64)?;
            self.heap_store(
                region,
                slot,
                Value::Simd {
                    dtype: Dtype::UInt8,
                    lanes: SimdLanes::Int(vec![i128::from(*byte)]),
                },
            );
        }
        Ok(())
    }

    /// A fresh byte allocation holding `bytes` plus a NUL terminator, as the
    /// pointer results of `strerror`/`getenv` (libc-owned, never freed).
    fn alloc_c_string(&mut self, bytes: &[u8]) -> Result<Value, RuntimeError> {
        let pointer = self.heap_alloc(bytes.len() as i64 + 1, 1)?;
        let mut image = bytes.to_vec();
        image.push(0);
        self.write_bytes(&pointer, &image)?;
        Ok(pointer)
    }

    fn env_lookup(&self, name: &[u8]) -> Option<Vec<u8>> {
        match self.host.env.get(name) {
            Some(Some(value)) => Some(value.clone()),
            Some(None) => None,
            None => std::env::var_os(OsStr::from_bytes(name)).map(|v| v.as_bytes().to_vec()),
        }
    }

    fn errno_pointer(&mut self) -> Result<Value, RuntimeError> {
        if let Some(allocation) = self.host.errno {
            return Ok(Value::Pointer {
                allocation,
                offset: 0,
            });
        }
        let pointer = self.heap_alloc(1, 4)?;
        if let Value::Pointer { allocation, .. } = pointer {
            self.host.errno = Some(allocation);
            let (region, slot) = self.heap_index(allocation, 0, 0)?;
            self.heap_store(
                region,
                slot,
                Value::Simd {
                    dtype: Dtype::Int32,
                    lanes: SimdLanes::Int(vec![0]),
                },
            );
        }
        Ok(pointer)
    }

    fn set_errno(&mut self, code: i32) -> Result<(), RuntimeError> {
        let Value::Pointer { allocation, .. } = self.errno_pointer()? else {
            unreachable!("errno slot is a pointer");
        };
        let (region, slot) = self.heap_index(allocation, 0, 0)?;
        self.heap_store(
            region,
            slot,
            Value::Simd {
                dtype: Dtype::Int32,
                lanes: SimdLanes::Int(vec![i128::from(code)]),
            },
        );
        Ok(())
    }

    /// The libc failure convention: set `errno` from the OS error and return -1.
    fn fail(&mut self, error: &std::io::Error) -> Result<Value, RuntimeError> {
        self.fail_code(error.raw_os_error().unwrap_or(EIO))
    }

    fn fail_code(&mut self, code: i32) -> Result<Value, RuntimeError> {
        self.set_errno(code)?;
        Ok(Value::Int(-1))
    }

    /// 0 on success, otherwise the failure convention.
    fn status(&mut self, result: std::io::Result<()>) -> Result<Value, RuntimeError> {
        match result {
            Ok(()) => Ok(Value::Int(0)),
            Err(error) => self.fail(&error),
        }
    }
}

/// glibc's `strerror` text for `code`: Rust sources its OS error message
/// from `strerror_r`, so stripping std's ` (os error N)` suffix yields the
/// same bytes the native executable prints.
pub fn strerror_text(code: i32) -> String {
    let text = std::io::Error::from_raw_os_error(code).to_string();
    match text.rfind(" (os error ") {
        Some(index) => text[..index].to_string(),
        None => text,
    }
}

fn null_pointer() -> Value {
    Value::Pointer {
        allocation: 0,
        offset: 0,
    }
}

fn os_path(bytes: &[u8]) -> &std::path::Path {
    std::path::Path::new(OsStr::from_bytes(bytes))
}

fn dirent_kind(kind: std::fs::FileType) -> u8 {
    if kind.is_dir() {
        DT_DIR
    } else if kind.is_symlink() {
        DT_LNK
    } else if kind.is_file() {
        DT_REG
    } else if kind.is_fifo() {
        DT_FIFO
    } else if kind.is_char_device() {
        DT_CHR
    } else if kind.is_block_device() {
        DT_BLK
    } else if kind.is_socket() {
        DT_SOCK
    } else {
        DT_UNKNOWN
    }
}

/// `scalar` in the representation `existing` already uses (a sized SIMD
/// lane keeps its dtype; a platform `Int` stays an `Int`).
fn retype_scalar(existing: &Value, scalar: i128) -> Value {
    match existing {
        Value::Simd { dtype, .. } => Value::Simd {
            dtype: *dtype,
            lanes: SimdLanes::Int(vec![scalar]),
        },
        Value::UInt(_) => Value::UInt(scalar as u64),
        _ => Value::Int(scalar as i64),
    }
}

fn int_arg(callee: &str, args: &[Value], index: usize) -> Result<i64, RuntimeError> {
    let Some(value) = args.get(index) else {
        return Err(RuntimeError::ArityMismatch {
            name: format!("external_call[\"{callee}\"]"),
            expected: index + 1,
            got: args.len(),
        });
    };
    c_integer(callee, index, value)
}

fn c_integer(callee: &str, index: usize, value: &Value) -> Result<i64, RuntimeError> {
    match value {
        Value::Int(n) => Ok(*n),
        Value::UInt(n) => Ok(*n as i64),
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::IntLiteral(literal) => literal.wrapping_signed(64).ok_or_else(|| {
            RuntimeError::TypeError(format!(
                "vm: external_call[\"{callee}\"] argument {} is out of range",
                index + 1
            ))
        }),
        Value::Simd {
            lanes: SimdLanes::Int(lanes),
            ..
        } if lanes.len() == 1 => Ok(lanes[0] as i64),
        other => Err(RuntimeError::TypeError(format!(
            "vm: external_call[\"{callee}\"] argument {} must be an integer scalar, got {}",
            index + 1,
            type_name(other)
        ))),
    }
}

/// A pointer argument: `None` for the null pointer, otherwise the handle
/// (a `CStringSlice` view unwraps to its single pointer field).
fn pointer_arg(callee: &str, args: &[Value], index: usize) -> Result<Option<Value>, RuntimeError> {
    let Some(value) = args.get(index) else {
        return Err(RuntimeError::ArityMismatch {
            name: format!("external_call[\"{callee}\"]"),
            expected: index + 1,
            got: args.len(),
        });
    };
    pointer_handle(callee, index, value)
}

fn pointer_handle(
    callee: &str,
    index: usize,
    value: &Value,
) -> Result<Option<Value>, RuntimeError> {
    match value {
        Value::Pointer { allocation: 0, .. } => Ok(None),
        Value::Pointer { .. } | Value::Ref { .. } => Ok(Some(value.clone())),
        Value::Struct { fields, .. } => {
            let mut pointers = fields
                .iter()
                .filter(|(_, field)| matches!(field, Value::Pointer { .. } | Value::Ref { .. }));
            match (pointers.next(), pointers.next()) {
                (Some((_, field)), None) => pointer_handle(callee, index, field),
                _ => Err(RuntimeError::TypeError(format!(
                    "vm: external_call[\"{callee}\"] argument {} is not a pointer-carrying view",
                    index + 1
                ))),
            }
        }
        other => Err(RuntimeError::TypeError(format!(
            "vm: external_call[\"{callee}\"] argument {} must be a pointer, got {}",
            index + 1,
            type_name(other)
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strerror_text_matches_glibc_wording() {
        assert_eq!(strerror_text(2), "No such file or directory");
        assert_eq!(strerror_text(9), "Bad file descriptor");
    }
}
