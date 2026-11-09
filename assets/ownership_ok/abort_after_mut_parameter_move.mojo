# A `mut` parameter moved before `abort(...)` is not uninitialized at the
# function's exit: the call returns `Never`, so no path reaches the exit
# with `s` moved. The pin prints `x`.
from std.os import abort


def take(var s: String):
    print(s)


def f(mut s: String):
    take(s^)
    abort("no")


def main():
    var s = String("x")
    if s.byte_length() > 5:
        f(s)
    print(s)
