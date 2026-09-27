# A surviving trait-bound module-level `def` derives its instances from the
# checked template (`docs/notes/instantiation-from-template.md`, class
# FunctionBody) through a keyword slice of a closed local, the stringify
# builtin, a whole rebinding of a local, a returned tuple display, an
# `external_call`, a direct call's result, and a raised `Error` of a built
# `String`. The bundled `os.rmdir` and `path.split` have this shape.

from std.ffi import external_call, get_errno
from std.os import rmdir
from std.os.path import split


def halves[T: Copyable & Writable](value: T, text: String) -> Tuple[String, String]:
    var middle = text.byte_length() // 2
    var head = String(text[byte=:middle])
    var tail = String(text[byte=middle:])
    if Bool(head):
        var trimmed = String(head.rstrip("a"))
        head = trimmed^
    return head, tail


def closed_file[T: Copyable & Writable](value: T, fd: Int) raises:
    var status = external_call["close", Int32](Int32(fd))
    if status != 0:
        var err = get_errno()
        raise Error(String("close failed: ") + String(fd) + " Err: " + String(err))


def main() raises:
    var head, tail = halves(1, String("banana"))
    print(head, tail)
    var h2, t2 = halves(String("s"), String("aaxyz"))
    print(h2, t2)
    var dir, name = split(String("/a/b//c"))
    print(dir, name)
    try:
        closed_file(1, -1)
    except e:
        print(e)
    try:
        closed_file(String("s"), -1)
    except e:
        print(e)
    try:
        rmdir(String("/nonexistent/mojito/a"))
    except e:
        print(e)
