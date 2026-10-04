# A `def` keyed on a type pack is served by its template: the body is
# checked once with the collector a pack of the symbolic `Ts` and each
# `args[i]` the dependent `Ts[i]`, MIR carries the loop over the pack's
# length as the `comptime_for` header, and the elaborator binds the pack from
# each call and unrolls the loop per instance. The length is spelled as the
# pin reads it at compile time: `args.__len__()`, `Ts.length`, or `len(Ts)`.
# An owned (`var`) collector is served the same way; its pack is destroyed
# last to first after the last element use. A pack spread whole into another
# served `def` (`tally(10, *rest)`, `drain(*items^)`) or into `print` is a
# call MIR carries with the collector as its spread argument; the elaborator
# passes the bound pack's elements.
def count[*Ts: Writable](*args: *Ts) -> Int:
    return len(args)


def tally[*Ts: Writable](first: Int, *rest: *Ts) -> Int:
    var total = first
    comptime for i in range(Ts.length):
        print(rest[i])
        total += 1
    return total


def second[*Ts: Writable](*items: *Ts):
    comptime for i in range(len(Ts)):
        comptime if i == 1:
            print("second:", items[i])
        else:
            print("other:", items[i])


def triangle[*Ts: Writable](*items: *Ts):
    comptime for i in range(items.__len__()):
        comptime for j in range(i + 1):
            print(i, j, items[j])


def joined[*Ts: Writable](*items: *Ts) -> String:
    var out = String("")
    comptime for i in range(items.__len__()):
        out += String(items[i])
        out += "|"
    return out


def pair[*Ts: Writable](*items: *Ts) -> Int:
    comptime if items.__len__() == 2:
        return 2
    return 0


def drain[*Ts: Writable & Movable](var *items: *Ts) -> Int:
    comptime for i in range(items.__len__()):
        print("drain:", items[i])
    return len(items)


def relay[*Ts: Writable](*rest: *Ts) -> Int:
    print(*rest, sep="/")
    return tally(10, *rest)


def forward_owned[*Ts: Writable & Movable](var *items: *Ts) -> Int:
    return drain(*items^)


def main():
    print(count(), count(1), count("a", 2.5, True))
    print(relay("r", 2, 3.5))
    print(forward_owned(True, "owned"))
    print(drain(1, "two", 3.5))
    print(tally(10, "x", 2))
    second(1, "two", 3.0)
    triangle("a", "b", "c")
    print(joined(1, "two", 3.5))
    print(joined())
    print(pair(1, "x"), pair(1))
