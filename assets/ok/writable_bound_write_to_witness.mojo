# A `write_to` through a `Writable` bound calls the conformer's witness, so a
# writer overriding `write` is re-entered by the conformer's own
# `writer.write(...)`, and by a scalar's digits, as at the pin.
struct Wrap(Writer):
    var text: String

    def __init__(out self):
        self.text = String()

    def write_string(mut self, s: StringSlice):
        self.text += "|"
        self.text += String(s)

    def write[*Ts: Writable](mut self, *args: *Ts):
        comptime for i in range(args.__len__()):
            self.text += "<"
            args[i].write_to(self)
            self.text += ">"


@fieldwise_init
struct P(Writable):
    var x: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("P", self.x)


def show[T: Writable](x: T, mut w: Wrap):
    x.write_to(w)


def main():
    var w = Wrap()
    show(P(1), w)
    print(w.text)
    var direct = Wrap()
    P(1).write_to(direct)
    print(direct.text)
    var scalars = Wrap()
    show(3, scalars)
    show(UInt8(9), scalars)
    show(True, scalars)
    show("lit", scalars)
    show(String("s"), scalars)
    print(scalars.text)
    var pack = Wrap()
    pack.write(1, "x", P(2))
    print(pack.text)
