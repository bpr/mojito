# A `T: Equatable` body's `==`/`!=` erased to a String receiver picks the
# `Self`-shaped overload at runtime even though String also declares
# same-arity view overloads; List membership and Dict keys use the same path.
def same[T: Equatable](a: T, b: T) -> Bool:
    return a == b
def differ[T: Equatable](a: T, b: T) -> Bool:
    return a != b
def main() raises:
    print(same(String("a"), String("a")), same(String("a"), String("b")))
    print(differ(String("a"), String("b")), same(3, 3))
    var xs: List[String] = [String("x"), String("y")]
    print(String("y") in xs)
    var d: Dict[String, Int] = {}
    d[String("k")] = 1
    print(d[String("k")])
