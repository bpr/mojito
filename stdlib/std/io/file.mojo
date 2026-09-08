"""Provides APIs to read and write files: `FileHandle` over a raw Unix
descriptor and `open`. Both are prelude names, as upstream."""

from std.collections.list import List
from std.ffi import c_int, c_size_t, c_ssize_t, external_call
from std.os import SEEK_SET, abort, makedirs
from std.os.path import dirname
from std.os.pathlike import PathLike as stdPathLike
from std.span import Span
from std.string import String, StringSpan
from std.sys._libc_errno import get_errno

# open() syscall flags (Linux; upstream selects them per platform).
comptime O_RDONLY = 0x0000
comptime O_WRONLY = 0x0001
comptime O_RDWR = 0x0002
comptime O_CREAT = 0x0040
comptime O_TRUNC = 0x0200
comptime O_APPEND = 0x0400
comptime O_CLOEXEC = 0x80000


# Open `path` in `mode` ("r", "w", "rw", or "a"), creating the parent
# directories of a written file, and return the descriptor.
def _open_file(path: String, mode: String) raises -> Int:
    var flags = 0
    var create_dirs = False
    if mode == "r":
        flags = O_RDONLY | O_CLOEXEC
    elif mode == "w":
        flags = O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC
        create_dirs = True
    elif mode == "rw":
        flags = O_RDWR | O_CREAT | O_CLOEXEC
        create_dirs = True
    elif mode == "a":
        flags = O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC
        create_dirs = True
    else:
        raise Error(
            String('invalid mode: "') + mode + '". Can only be one of: {"r", "w", "rw", "a"}'
        )

    if create_dirs:
        var parent = dirname(path)
        if Bool(parent):
            try:
                makedirs(parent, exist_ok=True)
            except e:
                raise Error(
                    String("unable to create directories '") + parent + "': " + String(e)
                )

    # int open(const char *path, int oflag, ...); mode 0o666 (modified by umask).
    var path_str = path
    var fd = external_call["open", c_int, num_fixed_args=2](
        path_str.as_c_string_slice(), c_int(flags), c_int(0o666)
    )
    if Int(fd) < 0:
        var err = get_errno()
        raise Error(String("Failed to open file '") + path + "': " + String(err))
    return Int(fd)


struct FileHandle(Defaultable, Movable, Writer):
    """File handle to an opened file."""

    var handle: Int

    def __init__(out self):
        self.handle = -1

    def __init__(out self, path: StringSpan, mode: StringSpan) raises:
        self.handle = _open_file(String(path), String(mode))

    def __deinit__(deinit self):
        try:
            self.close()
        except:
            pass

    def close(mut self) raises:
        if self.handle < 0:
            return
        var result = external_call["close", c_int](c_int(self.handle))
        if Int(result) < 0:
            var err = get_errno()
            # Still mark as closed even on error.
            self.handle = -1
            raise Error(String("Failed to close file: ") + String(err))
        self.handle = -1

    def read(self, size: Int = -1) raises -> String:
        var bytes = self.read_bytes(size)
        return String(from_utf8=bytes)

    def read_bytes(self, size: Int = -1) raises -> List[UInt8]:
        if self.handle < 0:
            raise Error("invalid file handle")
        # Start out with the correct size if we know it, otherwise use 256.
        var result = List[UInt8](unsafe_uninit_length=size if size >= 0 else 256)
        var fd = self._get_raw_fd()
        var num_read = 0
        while True:
            # A partial read is possible; EOF reads zero bytes.
            var chunk_bytes_to_read = len(result) - num_read
            var chunk_bytes_read = external_call["read", c_ssize_t](
                fd, result.unsafe_ptr().unsafe_offset(num_read), chunk_bytes_to_read
            )
            if chunk_bytes_read < 0:
                var err = get_errno()
                raise Error(String("Failed to read from file: ") + String(err))
            num_read += chunk_bytes_read
            if num_read == size or chunk_bytes_read == 0:
                result.shrink(num_read)
                break
            # Reading to EOF: keep going, taking bigger bites each time.
            if size < 0:
                result.resize(unsafe_uninit_length=num_read * 2)
        return result^

    def seek(self, offset: Int, whence: UInt8 = SEEK_SET) raises -> UInt64:
        if self.handle < 0:
            raise Error("invalid file handle")
        if Int(whence) > 2:
            abort("Second argument to `seek` must be between 0 and 2.")
        var fd = self._get_raw_fd()
        var pos = external_call["lseek", Int64](fd, Int64(offset), Int(whence))
        if Int(pos) < 0:
            var err = get_errno()
            raise Error(String("Failed to seek in file: ") + String(err))
        return UInt64(Int(pos))

    def write_once(mut self, bytes: Span[Byte, _]) raises -> Int:
        if self.handle < 0:
            raise Error("invalid file handle")
        var fd = self._get_raw_fd()
        var bytes_written = external_call["write", c_ssize_t](
            fd, bytes.unsafe_ptr(), len(bytes)
        )
        if bytes_written < 0:
            var err = get_errno()
            raise Error(String("Failed to write to file: ") + String(err))
        return bytes_written

    def write_all(mut self, bytes: Span[Byte, _]) raises:
        if self.handle < 0:
            raise Error("invalid file handle")
        var total_written = 0
        while total_written < len(bytes):
            var chunk_written = self.write_once(bytes[total_written:])
            if chunk_written == 0:
                raise Error("Write returned 0 bytes (file may be full or closed)")
            total_written += chunk_written

    # The `Writer` requirement: non-raising, so a failed write aborts.
    def write_bytes(mut self, bytes: Span[Byte, _]):
        if self.handle < 0:
            abort("invalid file handle in write_bytes()")
        var total_written = 0
        while total_written < len(bytes):
            var fd = self._get_raw_fd()
            var bytes_written = external_call["write", c_ssize_t](
                fd, bytes.unsafe_ptr().unsafe_offset(total_written), len(bytes) - total_written
            )
            if bytes_written < 0:
                abort("write() syscall failed")
            if bytes_written == 0:
                abort("write() returned 0 bytes (file may be full or closed)")
            total_written += bytes_written

    def write_string(mut self, string: String):
        self.write_bytes(string.as_bytes())

    def _write(self, ptr: Pointer[UInt8, _], len: Int) raises:
        if self.handle < 0:
            raise Error("invalid file handle")
        var fd = self._get_raw_fd()
        var total_written = 0
        while total_written < len:
            var current_ptr = ptr.unsafe_offset(total_written)
            var bytes_written = external_call["write", c_ssize_t](
                fd, current_ptr, len - total_written
            )
            if bytes_written < 0:
                var err = get_errno()
                raise Error(String("Failed to write to file: ") + String(err))
            if bytes_written == 0:
                raise Error("Write returned 0 bytes (file may be full or closed)")
            total_written += bytes_written

    def __enter__(var self) -> Self:
        return self^

    def _get_raw_fd(self) -> Int:
        return self.handle


def open[PathLike: stdPathLike](path: PathLike, mode: StringSlice) raises -> FileHandle:
    return FileHandle(path.__fspath__(), mode)
