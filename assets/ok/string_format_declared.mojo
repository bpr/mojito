# `format` on a String, a StringSpan, and a string literal, written through
# the bundled port of upstream's `_FormatUtils`, with a pack spread into each
# from a pack-keyed `def` served by its template.

comptime T = "m{}"


@fieldwise_init
struct P(Writable):
    var x: Int

    def write_repr_to(self, mut w: Some[Writer]):
        w.write("P[", self.x, "]")


def show_lit[*Ts: Writable](*a: *Ts) -> String:
    return "{} and {}".format(*a)


def show_s[*Ts: Writable](s: String, *a: *Ts) raises -> String:
    return s.format(*a)


def show_v[*Ts: Writable](*a: *Ts) raises -> String:
    return StringSpan("{1}-{0}").format(*a)


def show_comptime[*Ts: Writable](*a: *Ts) -> String:
    return T.format(*a)


def main() raises:
    print(show_lit(1, "x"))
    print(show_s("{} {}", 2.5, True))
    print(show_v(1, "y"))
    print(show_comptime(2))
    print("{0}{0}{{}}{1!r}".format("q", "w"))
    print("{!r} {!s} {}".format(P(2), "s", 2.5))
    var t = String("a{}b")
    print(t.format(7))
    var s = String("{} {}")
    try:
        print(s.format(1))
    except e:
        print("err:", e)
    try:
        print(s.format(1, "{"), String("{} {0}").format(1))
    except e:
        print("err:", e)
