# PROBE (divergence): converting a `List` to a `Span` copies its elements.
#
# `var s: Span[Dup, origin_of(xs)] = xs` copies the list into a temporary
# (each element's copy constructor runs) and destroys it. The pinned Mojo
# builds the view and copies nothing. An element read through the view
# (`print(s[0])`, `g(s[0])`) is lent, as a `List` element is.
#
# Observed 2026-10-25 against `Mojo 1.2.0.dev2026101705 (46f49943)`:
#   mojo:   D1 / g 1 / del 1 / end
#   mojito: copy 1 / del 1 / D1 / g 1 / del 1 / end
#
# When fixed: promote to `assets/ok`.
struct Dup(Copyable, Writable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, copy: Self):
        self.n = copy.n
        print("copy", self.n)

    def __deinit__(deinit self):
        print("del", self.n)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("D", self.n)


def g(d: Dup):
    print("g", d.n)


def main():
    var xs = List[Dup]()
    xs.append(Dup(1))
    var s: Span[Dup, origin_of(xs)] = xs
    print(s[0])
    g(s[0])
    print("end")
