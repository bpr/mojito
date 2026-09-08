"""glibc's x86-64 `struct stat` and the `__xstat`/`__lxstat` calls that fill it."""

from std.ffi import external_call
from std.time.time import _CTimeSpec

comptime dev_t = Int64
comptime mode_t = Int32
comptime nlink_t = Int64
comptime uid_t = Int32
comptime gid_t = Int32
comptime off_t = Int64
comptime blkcnt_t = Int64
comptime blksize_t = Int64


# Declared in glibc's field order so the native layout (declaration order,
# C padding) matches `struct stat`; the trailing reserved words are spelled
# as three fields because a Mojito `Array` is heap-backed. The VM fills the
# fields by name.
struct _c_stat(Copyable, Defaultable, Movable, Writable):
    var st_dev: dev_t
    var st_ino: Int64
    var st_nlink: nlink_t
    var st_mode: mode_t
    var st_uid: uid_t
    var st_gid: gid_t
    var _pad0: Int32
    var st_rdev: dev_t
    var st_size: off_t
    var st_blksize: blksize_t
    var st_blocks: blkcnt_t
    var st_atimespec: _CTimeSpec
    var st_mtimespec: _CTimeSpec
    var st_ctimespec: _CTimeSpec
    var st_birthtimespec: _CTimeSpec
    var _unused0: Int64
    var _unused1: Int64
    var _unused2: Int64

    def __init__(out self):
        self.st_dev = 0
        self.st_mode = 0
        self.st_nlink = 0
        self.st_ino = 0
        self.st_uid = 0
        self.st_gid = 0
        self._pad0 = 0
        self.st_rdev = 0
        self.st_size = 0
        self.st_blksize = 0
        self.st_blocks = 0
        self.st_atimespec = _CTimeSpec()
        self.st_mtimespec = _CTimeSpec()
        self.st_ctimespec = _CTimeSpec()
        self.st_birthtimespec = _CTimeSpec()
        self._unused0 = 0
        self._unused1 = 0
        self._unused2 = 0

    def write_to(self, mut writer: Some[Writer]):
        writer.write(
            "{\nst_dev: ", self.st_dev,
            ",\nst_mode: ", self.st_mode,
            ",\nst_nlink: ", self.st_nlink,
            ",\nst_ino: ", self.st_ino,
            ",\nst_uid: ", self.st_uid,
            ",\nst_gid: ", self.st_gid,
            ",\nst_rdev: ", self.st_rdev,
            ",\nst_size: ", self.st_size,
            ",\nst_blksize: ", self.st_blksize,
            ",\nst_blocks: ", self.st_blocks,
            ",\nst_atimespec: ", self.st_atimespec,
            ",\nst_mtimespec: ", self.st_mtimespec,
            ",\nst_ctimespec: ", self.st_ctimespec,
            ",\nst_birthtimespec: ", self.st_birthtimespec,
            "\n}",
        )


def _stat(var path: String) raises -> _c_stat:
    var stat = _c_stat()
    var err = external_call["__xstat", Int32](
        Int32(0), path.as_c_string_slice(), Pointer(to=stat)
    )
    if err == -1:
        raise Error(String("unable to stat '") + path + "'")
    return stat^


def _lstat(var path: String) raises -> _c_stat:
    var stat = _c_stat()
    var err = external_call["__lxstat", Int32](
        Int32(0), path.as_c_string_slice(), Pointer(to=stat)
    )
    if err == -1:
        raise Error(String("unable to lstat '") + path + "'")
    return stat^
