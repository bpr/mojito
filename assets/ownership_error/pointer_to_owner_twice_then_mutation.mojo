# expect: conflicts with live reference
# Shared pointer loans coexist with each other but still guard the owner:
# mutating the owner while either pointer lives is rejected.
def main():
    var xs = List[Int]()
    xs.append(1)
    var p = Pointer(to=xs)
    var q = Pointer(to=xs)
    xs.append(9)
    print(p[][0] + q[][0])
