# PROBE: a writer overriding `write` writes a scalar.
#
# A writer overriding `write` sees the pin's `Int.write_to` call
# `writer.write` for its digits, so the pin prints `<<>1x>`; Mojito formats
# the `Int` in the host and calls only `write_string`, printing `<1x>`
# (roadmap R423).
struct Buf(Writer):
    var text: String

    def __init__(out self):
        self.text = String()

    def write_string(mut self, string: StringSlice):
        self.text += string

    def write[*Ts: Writable](mut self, *args: *Ts):
        self.text += "<"
        comptime for i in range(args.__len__()):
            args[i].write_to(self)
        self.text += ">"


def main():
    var b = Buf()
    b.write(1, "x")
    print(b.text)
