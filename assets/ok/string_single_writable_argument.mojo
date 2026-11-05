# `String(x)` of one argument is upstream's overload resolution over the
# bundled initializers: a `StringLiteral` or `StaticString` picks its exact
# initializer, and anything else `Writable` the `*Ts: Writable` pack
# initializer, which writes the value's `write_to` into a fresh `String`.
@fieldwise_init
struct P(Writable):
    var x: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("P(", self.x, ")")


def main() raises:
    var s = String("abc")
    var span = StringSpan(s)
    var static: StaticString = "static"
    print(String(P(3)), String(42), String(2.5), String(False))
    print(String(DType.int16), String(Error("boom")), String((1, "a")))
    print(String(s), String(span), String(static), String("lit"))
    var pieces = [String(i) for i in range(3)]
    print(pieces[0] + pieces[1] + pieces[2])
