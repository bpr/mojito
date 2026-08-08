# Migrated literal operations on the nominal String: concatenation,
# comparison, membership, and in-place append, with mixed
# literal/nominal operands converting through the @implicit literal
# constructor.
def main():
    var a = String("foo")
    var b = String("bar")
    print(a + b)
    print(a + "!")
    print("<" + a)
    print(a == b, a == "foo", "foo" == a)
    print(a < b, "a" < b, a < "z")
    print("oo" in a, "zz" in a)
    var acc = String("")
    acc += a
    acc += "-"
    acc += b
    print(acc, len(acc))
