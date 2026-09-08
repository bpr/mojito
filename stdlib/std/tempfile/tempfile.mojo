"""Temporary file and directory creation (`gettempdir`, `mkdtemp`,
`TemporaryDirectory`)."""

from std.collections.list import List
from std.ffi import c_int, c_ssize_t, external_call
from std.optional import Optional
from std.os import abort, getenv, listdir, mkdir, remove, rmdir
from std.os.path import exists, isdir, isfile, islink, join
from std.pathlib import cwd

comptime TMP_MAX = 10000

# The random-name alphabet (upstream's `characters`).
comptime _NAME_CHARACTERS = "abcdefghijklmnopqrstuvwxyz0123456789_"


# Random names come from `/dev/urandom` until `std.random` lands (upstream
# draws them from `random_ui64`).
def _get_random_name(size: Int = 8) -> String:
    var device = String("/dev/urandom")
    var fd = external_call["open", c_int, num_fixed_args=2](
        device.as_c_string_slice(), c_int(0), c_int(0)
    )
    if Int(fd) < 0:
        abort("cannot open /dev/urandom")
    var buffer = String(unsafe_uninit_length=size)
    var got = external_call["read", c_ssize_t](fd, buffer.unsafe_ptr_mut(), UInt(size))
    var closed = external_call["close", c_int](fd)
    if got != size:
        abort("short read from /dev/urandom")
    var alphabet = String(_NAME_CHARACTERS)
    var name = String(capacity_bytes=size)
    var bytes = buffer.as_bytes()
    for i in range(size):
        var index = Int(bytes[i]) % alphabet.byte_length()
        name += String(alphabet[byte=index : index + 1])
    return name^


# The candidate temporary directories `_get_default_tempdir` tries, in order.
def _candidate_tempdir_list() -> List[String]:
    var dirlist = List[String]()
    var from_tmpdir = getenv("TMPDIR")
    if Bool(from_tmpdir):
        dirlist.append(from_tmpdir^)
    var from_temp = getenv("TEMP")
    if Bool(from_temp):
        dirlist.append(from_temp^)
    var from_tmp = getenv("TMP")
    if Bool(from_tmp):
        dirlist.append(from_tmp^)
    dirlist.append(String("/tmp"))
    dirlist.append(String("/var/tmp"))
    dirlist.append(String("/usr/tmp"))
    # As a last resort, the current directory if possible.
    try:
        var current = cwd()
        dirlist.append(current.path)
    except:
        pass
    return dirlist^


def _try_to_create_file(dir: String) -> Bool:
    for _ in range(TMP_MAX):
        var name = _get_random_name()
        var filename = join(dir, name)
        # Never overwrite an existing file.
        if exists(filename):
            continue
        # Verify write access in the target directory.
        try:
            with FileHandle(filename, "w"):
                pass
            remove(filename)
            return True
        except:
            if exists(filename):
                try:
                    remove(filename)
                except:
                    pass
            return False
    return False


# The default directory for temporary files: the first candidate a randomly
# named file can be created in.
def _get_default_tempdir() raises -> String:
    var dirlist = _candidate_tempdir_list()
    for dir_name in dirlist:
        if not isdir(dir_name):
            continue
        if _try_to_create_file(dir_name):
            return dir_name.copy()
    raise Error("No usable temporary directory found")


def gettempdir() -> Optional[String]:
    try:
        return Optional[String](_get_default_tempdir())
    except:
        return None


def mkdtemp(
    suffix: String = "", prefix: String = "tmp", dir: Optional[String] = None
) raises -> String:
    var final_dir = String("")
    if dir:
        final_dir = dir.value().copy()
    else:
        final_dir = _get_default_tempdir()
    for _ in range(TMP_MAX):
        var dir_name = join(final_dir, prefix + _get_random_name() + suffix)
        if exists(dir_name):
            continue
        try:
            mkdir(dir_name, mode=0o700)
            # The name may be relative (no `abspath`/`normpath` yet), as upstream.
            return dir_name^
        except:
            continue
    raise Error("Failed to create temporary file")


# Remove a directory tree (upstream's stand-in for `shutil.rmtree`).
def _rmtree(path: String, ignore_errors: Bool = False) raises:
    if islink(path):
        raise Error(String("`path`can not be a symbolic link: ") + path)
    for file_or_dir in listdir(path):
        var curr_path = join(path, file_or_dir)
        if isfile(curr_path):
            try:
                remove(curr_path)
            except e:
                if not ignore_errors:
                    raise e
            continue
        if isdir(curr_path):
            try:
                _rmtree(curr_path, ignore_errors)
            except e:
                if ignore_errors:
                    continue
                raise e
    try:
        rmdir(path)
    except e:
        if not ignore_errors:
            raise e


struct TemporaryDirectory:
    """Temporary directory that cleans up automatically: created on
    construction, removed with its contents when the context exits (even on
    an error; `ignore_cleanup_errors=True` suppresses cleanup failures)."""

    var name: String
    var _ignore_cleanup_errors: Bool

    def __init__(
        out self,
        suffix: String = "",
        prefix: String = "tmp",
        dir: Optional[String] = None,
        ignore_cleanup_errors: Bool = False,
    ) raises:
        self._ignore_cleanup_errors = ignore_cleanup_errors
        self.name = mkdtemp(suffix, prefix, dir)

    def __enter__(self) -> String:
        return self.name

    def __exit__(self) raises:
        _rmtree(self.name, ignore_errors=self._ignore_cleanup_errors)

    def __exit__(self, err: Error) -> Bool:
        try:
            self.__exit__()
            return True
        except:
            return False
