# A compile-time value is materialized at each read from its frozen fields,
# never by running its constructor again: `P.__init__` transforms its
# arguments, so a replay would read `q!!` and `28`.
struct P(Copyable, ImplicitlyCopyable, Writable):
    var name: String
    var n: Int

    def __init__(out self, name: String, n: Int):
        self.name = name + "!"
        self.n = n * 2

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self.name, ":", self.n)

comptime Q = P(String("q"), 7)
comptime T = (P(String("d"), 4), 5)
comptime PS = [P("a", 1), P("b", 2)]

def check(got: P, name: String, n: Int) raises:
    if got.name != name or got.n != n:
        raise Error("materialized " + got.name + ", expected " + name)

def main() raises:
    var q = Q
    check(q, "q!", 14)
    var again = Q
    check(again, "q!", 14)
    var t = T
    check(t[0], "d!", 8)
    var expected = 2
    comptime for p in PS:
        var y = p
        check(y, y.name, expected)
        expected += 2
    comptime r = P("r", 5)
    var z = r
    check(z, "r!", 10)
    print(q, t[0], t[1], z)
