# `Bool`'s `Writable` members are upstream's declared `write_to` and
# `write_repr_to` in `std.builtin.bool`: called directly into a writer that
# marks each piece it receives, and reached by `print`, `String`, `repr`,
# `Writer.write`, a t-string, a `Writable` bound, a reference, and the
# collections and structs that hold a `Bool`.


struct Marking(Writer):
    var text: String

    def __init__(out self):
        self.text = String()

    def write_string(mut self, string: StringSlice):
        self.text += "|"
        self.text += String(string)


@fieldwise_init
struct Pair(Writable):
    var flag: Bool
    var n: Int


def show[T: Writable](x: T) -> String:
    var s = String()
    x.write_to(s)
    return s


def written(ref b: Bool) -> String:
    var s = String()
    s.write(b, ";")
    return s


def main():
    var m = Marking()
    True.write_to(m)
    False.write_repr_to(m)
    print(m.text)
    var t = True
    var f = False
    print(t, f, sep=",", end="!\n")
    print(String(t), repr(f), String(f) + repr(t))
    ref r = t
    print(r, repr(r), String(r), written(r))
    print(show(f), t"{t} and {f}")
    var xs: List[Bool] = [True, False]
    print(xs, repr(xs))
    for ref x in xs:
        print(x, repr(x))
    print(Optional(True).value(), repr(Optional(False)))
    print((True, 1), Pair(True, 3), repr(Pair(False, 4)))
    print(Bool.MAX, Bool.MIN)
