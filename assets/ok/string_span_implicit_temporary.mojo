# A String temporary converts to a `StringSpan` parameter or binding through
# the view's `@implicit` `ref` constructor: the temporary materializes in a
# hidden owned slot the view borrows, and lives as long as the view (the
# same anchoring an explicit `StringSpan(String("abc"))` argument gets).
struct Sink(Movable):
    var total: Int

    def __init__(out self):
        self.total = 0

    def take(mut self, v: StringSpan):
        self.total += v.byte_length()

def make(n: Int) -> String:
    var s = String()
    var i = 0
    while i < n:
        s += "x"
        i += 1
    return s^

def byte_count(v: StringSpan) -> Int:
    return v.byte_length()

def main():
    print(byte_count(String("abc")), byte_count(make(3)))
    print(byte_count(String("a") + String("bc")))
    var v: StringSpan = String("abcd")
    var w: StringSpan = make(5)
    print(v.byte_length(), v, w.byte_length(), w)
    var sink = Sink()
    sink.take(String("abcde"))
    sink.take(make(2))
    sink.take(String("a") + String("b"))
    print(sink.total)
