# A user `Writer` implements `write_string(mut self, string: StringSlice)`:
# `write` formats each argument and hands its bytes to `write_string` as a
# borrowed view, and a literal or a `String` converts to the view when passed
# directly.
struct Sink(Writer):
    var seen: Int
    var text: String

    def __init__(out self):
        self.seen = 0
        self.text = String()

    def write_string(mut self, string: StringSlice):
        self.seen = self.seen + string.byte_length()
        self.text += String(string)


def main():
    var sink = Sink()
    sink.write("ab", 12)
    print(sink.seen, sink.text)
    sink.write_string("cd")
    var owned = String("ef")
    sink.write_string(owned)
    print(sink.seen, sink.text)
