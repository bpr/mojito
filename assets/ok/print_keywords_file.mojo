# `print` keywords: `sep`/`end` as literals, `String`, and `StringSpan`,
# `flush` (a no-op on the unbuffered captured stream), and `file` over a
# `FileDescriptor`.
from std.sys import stdout


def main():
    print("a", "b", "c", sep=", ")
    print("no newline", end="")
    print(" then one")
    var dash = String("-")
    print(1, 2, 3, sep=dash, end=String("!\n"))
    print(True, 7, 2.5, sep=StringSpan(" | "), flush=True)
    print("to stdout", file=stdout)
    print("x", "y", sep="", end="", file=FileDescriptor(1))
    print()
    print(sep="?", end="~\n")
