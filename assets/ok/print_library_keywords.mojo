# `print` is the prelude's library function, upstream's `std/io/io.mojo`
# `print[*Ts: Writable](*values, sep, end, flush, file)`: its keywords take
# a `String`, a `StaticString`, or a literal, `file=` another descriptor,
# a value longer than the stdout buffer is written through it, and a pack
# forwards whole into it.
from std.sys import stderr


@fieldwise_init
struct Q(Writable):
    var x: Int


def forward[*Ts: Writable](*args: *Ts):
    print(*args, sep="|")


def main():
    print("a", 1, 2.5, Q(3))
    print()
    print("x", "y", sep="-", end="!\n")
    var s = String(", ")
    print(1, 2, sep=s)
    var e: StaticString = "?\n"
    print(3, end=e)
    print(4, end="")
    print()
    var xs = [1, 2]
    print(xs, (1, "a"))
    print(Int8(3), True)
    print("err", 1, file=stderr)
    print("flushed", flush=True)
    var big = String("z") * 5000
    print(big.byte_length(), big[byte=0:3])
    print(big)
    forward(1, "two", Q(3))
