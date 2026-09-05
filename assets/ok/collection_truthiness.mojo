# Bare truthiness of a `Boolable` struct in every condition position:
# `if`, `while`, the conditional expression, and `not`.
def main():
    var xs = List[Int]()
    if xs:
        print("nonempty")
    else:
        print("empty")
    xs.append(1)
    if xs:
        print("nonempty")
    var s = String("")
    if not s:
        print("blank")
    var d = Dict[Int, Int]()
    print("has" if d else "none")
    while xs:
        _ = xs.pop()
    print(len(xs))
