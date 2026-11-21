# expect: conflicts with live reference
# Two pointers taken through the same `ref` binding while both stay live,
# and the binding read beside them. The pin runs it and prints 3, 1, 2;
# Mojito's pointer loans the whole referent, so the read conflicts, as a
# read beside a live mutable `Span` does (R1160).
def main():
    var xs = List[Int]()
    xs.append(1)
    xs.append(2)
    ref rx = xs
    var p = Pointer(to=rx)
    var q = Pointer(to=rx)
    print(p[][0] + q[][1])
    print(rx[0])
    print(p[][1])
