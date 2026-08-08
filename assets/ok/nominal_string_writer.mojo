# Writer conformance accepts either payload spelling: a write_string
# declaring the nominal String receives a materialized struct value,
# while the StringLiteral spelling keeps the builtin payload.
struct Sink(Writer):
    var seen: Int

    def __init__(out self):
        self.seen = 0

    def write_string(mut self, text: String):
        self.seen = self.seen + len(text)

struct LitSink(Writer):
    var seen: Int

    def __init__(out self):
        self.seen = 0

    def write_string(mut self, text: StringLiteral):
        self.seen = self.seen + len(text)

def main():
    var sink = Sink()
    sink.write("ab", 12)
    print(sink.seen)
    var lit = LitSink()
    lit.write("ab", 12)
    print(lit.seen)
