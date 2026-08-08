def main():
    var a = String("apple")
    var b = String("banana")
    var a2 = String("apple")
    print(a == a2, a == b, a != b)
    print(a < b, b < a, a <= a2, a >= a2)
    print(a.__hash__() == a2.__hash__(), a.__hash__() == b.__hash__())
    print(a)
    print(repr(b))
    var d = {String("one"): 1, String("two"): 2}
    try:
        print(d[String("two")], len(d))
    except:
        print("missing")
    print("done")
