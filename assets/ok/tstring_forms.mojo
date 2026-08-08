# The t-string surface over the lazy TString: adjacent merging, raw forms,
# empty templates, nesting, explicit String materialization, and a
# non-Copyable Writable place snapshotting as a creation-time string.
struct Point(Writable):
    var x: Int
    var y: Int

    def __init__(out self, x: Int, y: Int):
        self.x = x
        self.y = y

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Point(", self.x, ", ", self.y, ")")


def main():
    var value = 7
    print(t"answer: " t"{value}" t"!")
    print(rt"raw \n {value}")
    print(t"")
    var s: String = String(t"v={value}")
    print(s, len(s))
    var inner = 1
    print(t"outer={t"inner={inner}"}|")
    var stored = t"kept={value}"
    print(t"wrapped:{stored};")
    var point = Point(2, 5)
    print(t"point={point}, sum={point.x + point.y}")
