# A `def` keyed on a `Float64`, `StringLiteral`, or `UInt` value beside a
# type pack is served by its template, as one keyed on an `Int` is: an
# explicit application binds the value from its brackets and the pack from
# the collected arguments. A `def` keyed on such a value alone is served too.
def show[scale: Float64, *Ts: Writable](*args: *Ts):
    comptime for i in range(args.__len__()):
        print(args[i], scale)


def tag[label: StringLiteral, *Ts: Writable](*args: *Ts):
    comptime for i in range(args.__len__()):
        print(label, args[i])


def count[n: UInt, *Ts: Writable](*args: *Ts) -> UInt:
    return n + UInt(args.__len__())


def half[x: Float64]() -> Float64:
    return x / 2.0


def shout[s: StringLiteral]() -> String:
    return String(s) + "!"


def main():
    show[1.5](1, "a")
    tag["t"](2, 3.5)
    print(count[4](1, 2, 3))
    print(half[3.0]())
    print(shout["hi"]())
