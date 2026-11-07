# `List`, `Span`, and `Array` subscripts at a sized-integer `Indexer` index
# select the `__getitem__(ref self, idx: Some[Indexer])` overload, for a
# read, an in-place update, and an `Array` store through its reference.

def main():
    var l: List[Int] = [10, 20, 30]
    var s = Span(l)
    print(s[Int32(2)], s[UInt(0)], s[1])
    var a = Array[Int, 3](fill=4)
    a[1] = 8
    print(a[Int16(1)], a[0])
    a[Int8(2)] = 6
    print(a[2])
    print(l[Int32(1)], l[UInt8(2)], l[1])
    var t: List[String] = ["x", "y"]
    print(t[Int64(1)])
    l[Int32(0)] += 1
    print(l[0])
