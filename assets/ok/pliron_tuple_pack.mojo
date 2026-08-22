# Public Tuple construction and unpacking plus homogeneous `*args` at two
# arities. Pack elements relocate (never lifecycle-copy) into pack storage,
# so a double-destroyed or abandoned element surfaces in the sanitizer lane.
# (Unpacking a String element stays out: multi-target unpacking of a
# heap-owning element trips a pre-existing VM temp-lifetime bug.)
def sum(*values: Int) -> Int:
    var total: Int = 0
    for value in values:
        total = total + value
    return total


def labelled() -> Tuple[String, Int]:
    return (String("total"), 40 + 2)


def main():
    print(sum(), sum(5), sum(5, 6, 7))
    var pair: Tuple[String, Int] = labelled()
    print(pair[0], pair[1])
    var triple: Tuple[Int, Int, Int] = (1, 2, 3)
    var a: Int = 0
    var b: Int = 0
    var c: Int = 0
    a, b, c = triple
    print(a + b + c)
