# The bundled `os` proof subset. `abort` ends the program with an
# uncatchable trap under the CPU-default assertion configuration: the
# message crosses to the VM through the compiler-private `_mojito_abort`
# primitive, which never returns.

from std.string import String


def abort(message: String):
    _mojito_abort(message)
