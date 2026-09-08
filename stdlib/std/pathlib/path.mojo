"""Path manipulation: `Path`, a filesystem path with composition, existence
checks, file I/O, and directory listing over `std.os`; `cwd`."""

from std.collections.list import List
from std.ffi import external_call
from std.hashlib.hasher import Hasher
from std.os import PathLike, listdir as _listdir
from std.os.path import basename as _basename, exists as _exists, expanduser as _expanduser
from std.os.path import isdir as _isdir, isfile as _isfile
from std.span import Span

comptime DIR_SEPARATOR = "/"

# `getcwd`'s buffer size (upstream's `MAX_CWD_BUFFER_SIZE`).
comptime _MAX_CWD_BUFFER_SIZE = 1024


def cwd() raises -> Path:
    var buf = Array[UInt8, 1024](fill=0)
    var ptr = buf.unsafe_ptr()
    var res = external_call["getcwd", Pointer[UInt8, MutUntrackedOrigin]](
        ptr, UInt(_MAX_CWD_BUFFER_SIZE)
    )
    # A null result (upstream's `OptionalPointer`) reads as address 0.
    if Int(res) == 0:
        raise Error("unable to query the current directory")
    return Path(String(unsafe_from_utf8_ptr=ptr))


struct Path(
    Boolable,
    Comparable,
    Copyable,
    Equatable,
    Hashable,
    ImplicitlyCopyable,
    Movable,
    PathLike,
    Writable,
):
    """The Path object."""

    var path: String

    def __init__(out self) raises:
        var current = cwd()
        self.path = current.path

    # Not `@implicit` so that allocation is not implicit.
    def __init__(out self, path: StringSpan):
        self.path = String(path)

    @implicit
    def __init__(out self, var path: String):
        self.path = path^

    @implicit
    def __init__(out self, path: StringLiteral):
        self.path = String(path)

    def __truediv__(self, suffix: Self) -> Self:
        return self.__truediv__(StringSpan(suffix.path))

    def __truediv__(self, suffix: StringSpan) -> Self:
        var res = self.copy()
        res /= suffix
        return res^

    def __itruediv__(mut self, suffix: StringSpan):
        if self.path.endswith(DIR_SEPARATOR):
            self.path += String(suffix)
        else:
            self.path += String(DIR_SEPARATOR)
            self.path += String(suffix)

    def __bool__(self) -> Bool:
        return self.path.byte_length() > 0

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self.path)

    # `Path('...')`, the path string's repr inside.
    def write_repr_to(self, mut writer: Some[Writer]):
        writer.write("Path(")
        self.path.write_repr_to(writer)
        writer.write(")")

    def __fspath__(self) -> String:
        return self.path

    def __eq__(self, other: Self) -> Bool:
        return self.path == other.path

    def __eq__(self, other: StringSpan) -> Bool:
        return StringSpan(self.path) == other

    def __lt__(self, other: Self) -> Bool:
        return self.path < other.path

    def __hash__[H: Hasher](self, mut hasher: H):
        hasher.update(StringSpan(self.path))

    def exists(self) -> Bool:
        return _exists(self)

    def expanduser(self) raises -> Path:
        return Path(_expanduser(self))

    @staticmethod
    def home() raises -> Path:
        return Path(_expanduser(String("~")))

    def is_dir(self) -> Bool:
        return _isdir(self)

    def is_file(self) -> Bool:
        return _isfile(self)

    def read_text(self) raises -> String:
        with open(self, "r") as f:
            return f.read()

    def read_bytes(self) raises -> List[Byte]:
        with open(self, "r") as f:
            return f.read_bytes()

    def write_text[T: Writable](self, value: T) raises:
        with open(self, "w") as f:
            f.write(value)

    def write_bytes(self, bytes: Span[Byte, _]) raises:
        with open(self, "w") as f:
            f.write_bytes(bytes)

    def suffix(self) -> String:
        # +2 skips both `DIR_SEPARATOR` and a leading "." (`/a/.foo` has no
        # suffix; `/a/b.foo`'s is `.foo`).
        var start = self.path.rfind(DIR_SEPARATOR) + 2
        var i = self.path.rfind(".", start)
        if 0 < i < (self.path.byte_length() - 1):
            return String(self.path[byte=i:])
        return String("")

    def joinpath(self, *pathsegments: String) -> Path:
        if len(pathsegments) == 0:
            return self.copy()
        var result = self.copy()
        for segment in pathsegments:
            var view = StringSpan(segment)
            result /= view
        return result^

    def listdir(self) raises -> List[Path]:
        var ls = _listdir(self)
        var res = List[Path](capacity=len(ls))
        for i in range(len(ls)):
            res.append(Path(ls[i]))
        return res^

    def name(self) -> String:
        return _basename(self)

    def parts(self) -> List[String]:
        return self.path.split(DIR_SEPARATOR)
