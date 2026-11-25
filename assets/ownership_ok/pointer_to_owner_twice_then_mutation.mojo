# Two pointers to one owner coexist, and the owner's mutation while both
# live runs, as at the pin: a pointer names its owner's origin and borrows
# nothing exclusively (R1160).
def main():
    var xs = List[Int]()
    xs.append(1)
    var p = Pointer(to=xs)
    var q = Pointer(to=xs)
    xs.append(9)
    print(p[][0] + q[][0])
