"""The C library's `errno`: reading, setting, and rendering error codes."""


def _errno_ptr() -> Pointer[Int32, MutUntrackedOrigin]:
    return external_call["__errno_location", Pointer[Int32, MutUntrackedOrigin]]()


def get_errno() -> ErrNo:
    var ptr = _errno_ptr()
    return ErrNo(ptr[])


def set_errno(errno: ErrNo):
    var ptr = _errno_ptr()
    ptr[] = errno.value


# A libc error code. `String(err)` renders glibc's `strerror` text, as
# upstream. Upstream's named constants (`ErrNo.ENOENT`, ...) are struct-level
# `comptime` values Mojito's associated-constant evaluator does not fold yet;
# compare against `ErrNo(2)` meanwhile.
struct ErrNo(Copyable, Equatable, Movable, Writable):
    var value: Int32

    def __init__(out self, value: Int32):
        self.value = value

    def __init__(out self, value: Int):
        self.value = Int32(value)

    def write_to(self, mut writer: Some[Writer]):
        var ptr = external_call["strerror", Pointer[Byte, MutUntrackedOrigin]](self.value)
        writer.write(String(unsafe_from_utf8_ptr=ptr))

    def __eq__(self, other: Self) -> Bool:
        return Bool(self.value == other.value)

    def __ne__(self, other: Self) -> Bool:
        return Bool(self.value != other.value)
