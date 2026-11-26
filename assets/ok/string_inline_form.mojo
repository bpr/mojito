# Upstream's inline (SSO) `String`: up to 23 bytes held in the String's own
# three words, so appends that fit write in place and a view over the bytes
# survives them; a copy of an inline String is independent, and growth past
# 23 bytes spills to a heap buffer of at least 48.


def raises_short() raises:
    raise Error(String("short ") + "msg")


def main() raises:
    var s = String("ab") + "c"
    for g in s:
        s += "!!!"
        print(g)
    print(s, s.byte_length(), s.capacity_bytes())

    print(String().capacity_bytes(), String(capacity_bytes=4).capacity_bytes())
    print(String("").capacity_bytes(), String(capacity_bytes=100).capacity_bytes())
    var grown = String("abc")
    print(grown.capacity_bytes())
    for _ in range(18):
        grown += "x"
    print(grown.byte_length(), grown.capacity_bytes())
    grown += "yyyyy"
    print(grown, grown.byte_length(), grown.capacity_bytes())

    var t = String(capacity_bytes=4)
    t += "abc"
    var u = t.copy()
    u += "d"
    print(t, u, t == "abc", u > t, t.capacity_bytes(), u.capacity_bytes())
    print(String(1, 2.5, "x", sep=","), String(12345).capacity_bytes())

    var xs = List[String]()
    xs.append("a")
    xs.append(String("bc") + "d")
    xs[0] += "zz"
    xs[1] += "e"
    print(xs[0], xs[1], xs[1].capacity_bytes())

    var empty = String()
    empty += "hello"
    print(empty, empty.byte_length(), empty.upper(), Int(String("4") + "2") + 1)

    try:
        raises_short()
    except e:
        print("caught:", e)
