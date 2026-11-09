# An `if` arm that ends in `abort(...)` reads a value defined before the
# `if`: it stays alive into the arm rather than being destroyed before the
# `if`, so the abort reports the whole message, as at the pin.
# expect: abort: bad value -1
from std.os import abort


def check(n: Int) -> Int:
    var message = String("bad value ")
    message += String(n)
    if n < 0:
        abort(message)
    return n * 10


def main():
    print(check(4))
    print(check(-1))
