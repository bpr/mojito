# Two pointers taken through the same `ref` binding while both stay live:
# each pointer reborrows through the binding instead of competing with the
# binding's own loan, and the binding itself remains readable beside them.
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
