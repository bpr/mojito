def make() -> List[Int]:
    var xs = List[Int]()
    xs.append(4)
    xs.append(5)
    return xs^

def words() -> List[String]:
    var xs = List[String]()
    xs.append("a")
    xs.append("bc")
    return xs^

def nested() -> List[List[Int]]:
    var xs = List[List[Int]]()
    var a = List[Int]()
    a.append(1)
    a.append(2)
    var b = List[Int]()
    b.append(3)
    xs.append(a^)
    xs.append(b^)
    return xs^

def total[xs: List[Int]]() -> Int:
    var result = 0
    for x in materialize[xs]():
        result += x
    return result

def main():
    comptime xs = make()
    comptime copied = xs.copy()
    comptime ss = words()
    comptime ns = nested()
    var a = materialize[xs]()
    var b = materialize[xs]()
    a[0] = 8
    print(a[0], b[0], len(b), total[xs]())
    var c = materialize[copied]()
    var s = materialize[ss]()
    var n = materialize[ns]()
    print(c[0], s[1], len(s), n[0][1], len(n[1]))
    var sum = 0
    comptime for x in xs:
        sum += x
    print(sum)
