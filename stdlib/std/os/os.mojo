"""Directory and file operations (`listdir`, `mkdir`, `remove`, ...)."""

from std.collections.list import List
from std.ffi import external_call, get_errno
from .path import exists, isdir, split
from .pathlike import PathLike as stdPathLike

comptime sep = "/"

comptime SEEK_SET: UInt8 = 0
comptime SEEK_CUR: UInt8 = 1
comptime SEEK_END: UInt8 = 2

# glibc's `struct dirent` places `d_name` at byte 19 (`d_ino`, `d_off`,
# `d_reclen`, `d_type`). Upstream reads the entry through a `_dirent_linux`
# struct whose `name: Array[c_char, 256]` is inline; a Mojito `Array` is
# heap-backed, so the entry is read as bytes instead.
comptime _DIRENT_NAME_OFFSET = 19


# A directory stream (`opendir`/`readdir`/`closedir`).
struct _DirHandle(Movable):
    var _handle: Pointer[UInt8, MutUntrackedOrigin]

    def __init__(out self, var path: String) raises:
        if not isdir(path):
            raise Error(String("the directory '") + path + "' does not exist")
        var handle = external_call["opendir", Pointer[UInt8, MutUntrackedOrigin]](
            path.as_c_string_slice()
        )
        if Int(handle) == 0:
            var err = get_errno()
            raise Error(
                String("unable to open the directory '") + path + "' Err: " + String(err)
            )
        self._handle = handle

    def __deinit__(deinit self):
        var closed = external_call["closedir", Int32](self._handle)

    def list(self) -> List[String]:
        var res = List[String]()
        while True:
            var ep = external_call["readdir", Pointer[UInt8, MutUntrackedOrigin]](
                self._handle
            )
            if Int(ep) == 0:
                break
            var name = String(unsafe_from_utf8_ptr=ep.unsafe_offset(_DIRENT_NAME_OFFSET))
            if name == "." or name == "..":
                continue
            res.append(name^)
        return res^


def listdir[PathLike: stdPathLike](path: PathLike) raises -> List[String]:
    var dir = _DirHandle(path.__fspath__())
    return dir.list()


# Mojito's abort: an uncatchable trap carrying the message (the compiler
# crossing `_mojito_abort`).
def abort(message: String):
    _mojito_abort(message)


def remove[PathLike: stdPathLike](path: PathLike) raises:
    var fspath = path.__fspath__()
    var error = external_call["unlink", Int32](fspath.as_c_string_slice())
    if error != 0:
        var err = get_errno()
        raise Error(String("Can not remove file: ") + fspath + " Err: " + String(err))


def unlink[PathLike: stdPathLike](path: PathLike) raises:
    remove(path.__fspath__())


def mkdir[PathLike: stdPathLike](path: PathLike, mode: Int = 0o777) raises:
    var fspath = path.__fspath__()
    var error = external_call["mkdir", Int32](fspath.as_c_string_slice(), mode)
    if error != 0:
        var err = get_errno()
        raise Error(String("Can not create directory: ") + fspath + " Err: " + String(err))


# The recursive body of `makedirs` over a concrete path: a bound-generic def
# whose template body calls itself with a concrete argument would mint its
# own specialization while checking, so the recursion lives here.
def _makedirs(path: String, mode: Int, exist_ok: Bool) raises:
    var head, tail = split(path)
    if not Bool(tail):
        head, tail = split(head)
    if Bool(head) and Bool(tail) and not exists(head):
        try:
            _makedirs(head, 0o777, exist_ok)
        except:
            pass
        if tail == ".":
            return
    try:
        mkdir(path, mode)
    except e:
        if not exist_ok:
            raise Error(
                String(e) + "\nset `makedirs(path, exist_ok=True)` to allow existing dirs"
            )
        if not isdir(path):
            raise Error(String("path not created: ") + path + "\n" + String(e))


def makedirs[
    PathLike: stdPathLike
](path: PathLike, mode: Int = 0o777, exist_ok: Bool = False) raises -> None:
    _makedirs(path.__fspath__(), mode, exist_ok)


def rmdir[PathLike: stdPathLike](path: PathLike) raises:
    var fspath = path.__fspath__()
    var error = external_call["rmdir", Int32](fspath.as_c_string_slice())
    if error != 0:
        var err = get_errno()
        raise Error(String("Can not remove directory: ") + fspath + " Err: " + String(err))


def removedirs[PathLike: stdPathLike](path: PathLike) raises -> None:
    rmdir(path)
    var head, tail = split(path)
    if not Bool(tail):
        head, tail = split(head)
    while Bool(head) and Bool(tail):
        try:
            rmdir(head)
        except:
            break
        head, tail = split(head)
