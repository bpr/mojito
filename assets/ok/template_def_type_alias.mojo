# A surviving trait-bound module-level `def` spelling a type alias derives
# its instances from the checked template
# (`docs/notes/instantiation-from-template.md`): the alias `c_int = Int32`
# expands with the same identities in every clone, in an `external_call`
# result type, a construction, and a local's annotation.

from std.ffi import c_int, external_call


def closed_status[T: Copyable & Writable](value: T, fd: Int) -> Int:
    var status: c_int = external_call["close", c_int](c_int(fd))
    var zero = c_int(0)
    if status != zero:
        return Int(status)
    return 0


def main():
    print(closed_status(1, -1))
    print(closed_status(String("s"), -1))
