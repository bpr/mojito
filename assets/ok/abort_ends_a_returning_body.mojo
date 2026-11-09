# A value-returning body whose last statement is `abort(...)` has no
# fall-through path: `std.os.abort` returns `Never`, and a call whose result
# is `Never` ends its path, as at the pin, which prints 1.
from std.os import abort


def f(x: Int) -> Int:
    if x > 0:
        return 1
    abort("no")


def main():
    print(f(1))
