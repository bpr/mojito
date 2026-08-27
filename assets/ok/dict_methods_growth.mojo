# Dict method growth: pop (raising and defaulted), popitem (LIFO),
# setdefault (ref result), update, merge (__or__), __bool__, order-insensitive
# __eq__/__ne__, clear, and the capacity constructor — identical output on
# both compilers, including post-removal lookups through the rebuilt index.
def main() raises:
    var d: Dict[Int, Int] = Dict[Int, Int]()
    var i = 0
    while i < 12:
        d[i] = i * 10
        i += 1
    print(d.pop(3))
    print(d.pop(99, -1))
    print(d.pop(0, -1))
    print(len(d))
    var last = d.popitem()
    print(last.key, last.value)
    print(d.setdefault(1, 111))
    print(d.setdefault(50, 500))
    print(len(d), d[50])
    var e: Dict[Int, Int] = Dict[Int, Int]()
    e[50] = 555
    e[60] = 600
    d.update(e)
    print(d[50], d[60])
    var merged = d | e
    print(len(merged), merged[60])
    var empty0: Dict[Int, Int] = Dict[Int, Int]()
    print(Bool(d), Bool(empty0))
    var a: Dict[Int, Int] = {1: 10, 2: 20}
    var b: Dict[Int, Int] = {2: 20, 1: 10}
    print(Bool(a == b), Bool(a != b))
    b[2] = 99
    print(Bool(a == b))
    a.clear()
    print(len(a), Bool(a))
    var c: Dict[Int, Int] = Dict[Int, Int](capacity=100)
    c[1] = 1
    print(len(c))
    var total = 0
    for k in d:
        total += d[k]
    print(total)
    try:
        var v = d.pop(12345)
        print(v)
    except:
        print("caught")
    try:
        var empty1: Dict[Int, Int] = Dict[Int, Int]()
        var it = empty1.popitem()
        print(it.key)
    except:
        print("caught empty")
