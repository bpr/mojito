# A pack-keyed `def` that spreads its collector into `write` is served by its
# template: on a `String` the call selects `String.write[*Ts]`, and through
# `W: Writer` or `Some[Writer]` it dispatches to the conformer's `write`, the
# trait's default where the conformer declares only `write_string`.
struct Buf(Writer):
    var text: String

    def __init__(out self):
        self.text = String()

    def write_string(mut self, string: StringSlice):
        self.text += string


def render[*Ts: Writable](*a: *Ts) -> String:
    var s = String()
    s.write(*a)
    return s


def into[W: Writer, *Ts: Writable](mut w: W, *a: *Ts):
    w.write(*a)


def via_some[*Ts: Writable](mut w: Some[Writer], *a: *Ts):
    w.write(*a)


def main():
    print(render(1, "x", 2.5))
    var s = String()
    into(s, 1, "x", 2.5)
    print(s)
    var b = Buf()
    into(b, 1, "x", 2.5)
    via_some(b, True, 7)
    print(b.text)
