# A `Writer` conformer may override the trait's default `write`, which a
# direct call and a call through `W: Writer` both reach; a conformer
# declaring only `write_string` takes the default, through a bound and
# through `Some[Writer]` alike.
struct Dots(Writer):
    var text: String

    def __init__(out self):
        self.text = String()

    def write_string(mut self, string: StringSlice):
        self.text += string

    def write[*Ts: Writable](mut self, *args: *Ts):
        self.text += "<"
        comptime for i in range(args.__len__()):
            self.text += "."
        self.text += ">"


struct Buf(Writer):
    var text: String

    def __init__(out self):
        self.text = String()

    def write_string(mut self, string: StringSlice):
        self.text += string


def pair[W: Writer](mut w: W):
    w.write(7, "-", 8)


def triple(mut w: Some[Writer]):
    w.write(True, "/", 2.5, "/", 3)


def main():
    var d = Dots()
    d.write(1, "x")
    pair(d)
    print(d.text)
    var b = Buf()
    b.write(1, "x")
    pair(b)
    triple(b)
    print(b.text)
