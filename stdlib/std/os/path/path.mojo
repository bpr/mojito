"""Path predicates and string manipulation (`os.path`)."""

from std.stat import S_ISDIR, S_ISLNK, S_ISREG
from std.string import String
from ..pathlike import PathLike as stdPathLike
from .._linux_x86 import _lstat, _stat
from ..env import getenv

# `os.sep`; spelled locally so this module does not import `os` (which
# imports it).
comptime sep = "/"


def _get_stat_st_mode(var path: String) raises -> Int:
    var record = _stat(path^)
    return Int(record.st_mode)


def _get_lstat_st_mode(var path: String) raises -> Int:
    var record = _lstat(path^)
    return Int(record.st_mode)


# `~` expands through `HOME`; `~user` needs `getpwnam`, which Mojito does
# not bind, and expands to nothing (the caller keeps the path unchanged).
def _user_home_path(path: String) -> String:
    var user_end = path.find(sep, 1)
    if user_end < 0:
        user_end = path.byte_length()
    if path.byte_length() > 1 and user_end > 1:
        return String("")
    return getenv("HOME")


def join(var path: String, *paths: String) -> String:
    var joined_path = path^
    for cur_path in paths:
        if cur_path.startswith(sep):
            joined_path = cur_path
        elif not Bool(joined_path) or joined_path.endswith(sep):
            joined_path += cur_path
        else:
            joined_path += String(sep) + cur_path
    return joined_path^


def expanduser[PathLike: stdPathLike, //](path: PathLike) raises -> String:
    var fspath = path.__fspath__()
    if not fspath.startswith("~"):
        return fspath
    var userhome = _user_home_path(fspath)
    if not Bool(userhome):
        return fspath
    var path_split = fspath.split(sep, 1)
    if len(path_split) == 2:
        var rest = String(path_split[1])
        return join(userhome, rest)
    return userhome^


def isdir[PathLike: stdPathLike, //](path: PathLike) -> Bool:
    var fspath = path.__fspath__()
    try:
        var st_mode = _get_stat_st_mode(fspath)
        if S_ISDIR(st_mode):
            return True
        return S_ISLNK(st_mode) and S_ISDIR(_get_lstat_st_mode(fspath^))
    except:
        return False


def isfile[PathLike: stdPathLike, //](path: PathLike) -> Bool:
    var fspath = path.__fspath__()
    try:
        var st_mode = _get_stat_st_mode(fspath)
        if S_ISREG(st_mode):
            return True
        return S_ISLNK(st_mode) and S_ISREG(_get_lstat_st_mode(fspath))
    except:
        return False


def islink[PathLike: stdPathLike, //](path: PathLike) -> Bool:
    try:
        return S_ISLNK(_get_lstat_st_mode(path.__fspath__()))
    except:
        return False


def _all_separators(text: String) -> Bool:
    var bytes = text.as_bytes()
    var i = 0
    while i < len(bytes):
        if bytes[i] != UInt8(47):
            return False
        i += 1
    return True


def dirname[PathLike: stdPathLike, //](path: PathLike) -> String:
    var fspath = path.__fspath__()
    var i = fspath.rfind(sep) + 1
    var head = String(fspath[byte=:i])
    if Bool(head) and not _all_separators(head):
        return String(head.rstrip(sep))
    return head^


def exists[PathLike: stdPathLike, //](path: PathLike) -> Bool:
    try:
        var mode = _get_stat_st_mode(path.__fspath__())
        return True
    except:
        return False


def lexists[PathLike: stdPathLike, //](path: PathLike) -> Bool:
    try:
        var mode = _get_lstat_st_mode(path.__fspath__())
        return True
    except:
        return False


def getsize[PathLike: stdPathLike, //](path: PathLike) raises -> Int:
    var record = _stat(path.__fspath__())
    return Int(record.st_size)


def is_absolute[PathLike: stdPathLike, //](path: PathLike) -> Bool:
    return path.__fspath__().startswith(sep)


def split[PathLike: stdPathLike, //](path: PathLike) -> Tuple[String, String]:
    var fspath = path.__fspath__()
    var i = fspath.rfind(sep) + 1
    var head = String(fspath[byte=:i])
    var tail = String(fspath[byte=i:])
    if Bool(head) and not _all_separators(head):
        var stripped = String(head.rstrip(sep))
        head = stripped^
    return head, tail


def basename[PathLike: stdPathLike, //](path: PathLike) -> String:
    var fspath = path.__fspath__()
    var i = fspath.rfind(sep) + 1
    var head = String(fspath[byte=i:])
    if Bool(head) and not _all_separators(head):
        return String(head.rstrip(sep))
    return head^


def _split_extension(
    path: String, sep: String, alt_sep: String, extension_sep: String
) raises -> Tuple[String, String]:
    var head_end = path.rfind(sep)
    if Bool(alt_sep):
        head_end = max(head_end, path.rfind(alt_sep))
    var file_end = path.rfind(extension_sep)
    if file_end > head_end:
        var file_start = head_end + 1
        var bytes = path.as_bytes()
        var extension_bytes = extension_sep.as_bytes()
        var extension_byte = extension_bytes[0]
        while file_start < file_end:
            if bytes[file_start] != extension_byte:
                return String(path[byte=:file_end]), String(path[byte=file_end:])
            file_start += 1
    return path, String("")


def split_extension[
    PathLike: stdPathLike, //
](path: PathLike) raises -> Tuple[String, String]:
    return _split_extension(path.__fspath__(), String(sep), String(""), String("."))


def splitroot[
    PathLike: stdPathLike, //
](path: PathLike) -> Tuple[String, String, String]:
    var p = path.__fspath__()
    var empty = String("")
    var length = p.byte_length()
    var separator = String(sep)
    if length < 1 or String(p[byte=:1]) != separator:
        return empty, String(""), p
    elif (
        length < 2
        or String(p[byte=1:2]) != separator
        or (length >= 3 and String(p[byte=2:3]) == separator)
    ):
        var rest = String(p[byte=1:])
        return empty, separator, rest
    else:
        var root = String(p[byte=:2])
        var rest = String(p[byte=2:])
        return empty, root, rest


# Shell special variables: `*#$@!?-` and the digits.
def _is_shell_special_variable(byte: Byte) -> Bool:
    var b = Int(byte)
    return (
        b == 42
        or b == 35
        or b == 36
        or b == 64
        or b == 33
        or b == 63
        or b == 45
        or (48 <= b and b <= 57)
    )


def _is_alphanumeric(byte: Byte) -> Bool:
    var b = Int(byte)
    return (
        b == 95
        or (48 <= b and b <= 57)
        or (97 <= b and b <= 122)
        or (65 <= b and b <= 90)
    )


# The variable name starting at byte `start` of `path` and the number of
# bytes it spans (upstream's `_parse_variable_name` over the remaining bytes).
def _parse_variable_name(path: String, start: Int) -> Tuple[String, Int]:
    var bytes = path.as_bytes()
    var n = len(bytes)
    if bytes[start] == UInt8(123):
        if (
            n - start > 2
            and _is_shell_special_variable(bytes[start + 1])
            and Bool(bytes[start + 2] == UInt8(125))
        ):
            return String(path[byte=start + 1:start + 2]), 3
        var i = 1
        while start + i < n:
            if bytes[start + i] == UInt8(125):
                return String(path[byte=start + 1:start + i]), i + 1
            i += 1
        return String(path[byte=start + 1:start + i]), i
    elif _is_shell_special_variable(bytes[start]):
        return String(path[byte=start:start + 1]), 1
    var i = 0
    while start + i < n and _is_alphanumeric(bytes[start + i]):
        i += 1
    return String(path[byte=start:start + i]), i


def expandvars[PathLike: stdPathLike, //](path: PathLike) -> String:
    var path_str = path.__fspath__()
    var bytes = path_str.as_bytes()
    var n = len(bytes)
    var buf = String()
    var i = 0
    var j = 0
    while j < n:
        if Bool(bytes[j] == UInt8(36)) and j + 1 < n:
            if not Bool(buf):
                buf.reserve_bytes(2 * n)
            buf.write_string(String(path_str[byte=i:j]))
            var name, length = _parse_variable_name(path_str, j + 1)
            if name.startswith("{") or name == "":
                buf.write_string(String(path_str[byte=j:j + length + 1]))
            elif _is_shell_special_variable(bytes[j + 1]):
                buf.write_string(String(path_str[byte=j:j + 2]))
            else:
                var value = getenv(name)
                if value != "":
                    buf.write_string(value)
                else:
                    buf.write_string(String(path_str[byte=j:j + length + 1]))
            j += length
            i = j + 1
        j += 1
    if not Bool(buf):
        return path_str^
    var rest = String(path_str[byte=i:])
    buf.write_string(rest)
    return buf^
