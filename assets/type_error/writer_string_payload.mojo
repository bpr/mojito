# expect: does not conform to trait 'Writer'
# `Writer.write_string` takes the borrowed view (`StringSlice`), as upstream;
# a conformer declaring an owned `String` payload does not implement it.
struct Buf(Writer):
    var text: String

    def __init__(out self):
        self.text = String()

    def write_string(mut self, s: String):
        self.text += s


def main():
    var b = Buf()
    b.write("a", 1)
    print(b.text)
