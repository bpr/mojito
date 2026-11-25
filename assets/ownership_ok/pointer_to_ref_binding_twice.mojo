# Two pointers taken through the same `ref` binding while both stay live,
# and the binding read beside them, run as at the pin: 3, 1, 2 (R1160).
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
