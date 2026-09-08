# `std.os.path`: the string functions (`join`, `split`, `basename`,
# `dirname`, `split_extension`, `splitroot`, `is_absolute`, `expandvars`,
# `expanduser`) and the stat-backed predicates (`exists`, `isdir`,
# `isfile`, `islink`, `getsize`) over paths every Linux host has.
from std.os import getenv, setenv
from std.os.path import (
    basename,
    dirname,
    exists,
    expanduser,
    expandvars,
    getsize,
    is_absolute,
    isdir,
    isfile,
    islink,
    join,
    lexists,
    split,
    split_extension,
    splitroot,
)


def main() raises:
    print(join(String("usr"), String("lib"), String("x.so")))
    print(join(String("usr/"), String("lib")), join(String("usr"), String("/abs")))
    print(dirname(String("/usr/lib/x.so")), basename(String("/usr/lib/x.so")))
    print(dirname(String("/")), basename(String("/usr/lib/")), dirname(String("x.so")))
    var head, tail = split(String("/usr/lib/x.so"))
    print(head, tail)
    var root, ext = split_extension(String("archive.tar.gz"))
    print(root, ext)
    var hidden_root, hidden_ext = split_extension(String(".bashrc"))
    print(hidden_root, hidden_ext)
    var drive, share, rest = splitroot(String("//x/y"))
    print(drive, share, rest)
    var drive2, share2, rest2 = splitroot(String("///x"))
    print(drive2, share2, rest2)
    print(is_absolute(String("/x")), is_absolute(String("x")))
    print(exists(String("/")), isdir(String("/")), isfile(String("/")), islink(String("/")))
    print(exists(String("/definitely/missing")), lexists(String("/definitely/missing")))
    print(isfile(String("/dev/null")), getsize(String("/dev/null")))
    var set_ok = setenv("MOJITO_FIXTURE_VAR", "val")
    print(expandvars(String("$MOJITO_FIXTURE_VAR/x")), expandvars(String("${MOJITO_FIXTURE_VAR}y")))
    print(expandvars(String("$MOJITO_FIXTURE_UNSET/x")), expandvars(String("plain")))
    print(expanduser(String("~/x")) == join(getenv("HOME"), String("x")))
    print(expanduser(String("plain")))
