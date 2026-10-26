# A nested `def` or lambda capturing a reference binding by `imm` or `var`
# borrows or copies its referent through its closure's storage.
def main():
    var xs: List[Int] = [1, 2]
    var total = 0
    for a in xs:
        def f() {imm a} -> Int:
            return a * 10
        def h() {var a} -> Int:
            return a * 100
        total = total + f() + h()
    ref r = xs[0]
    total = total + (lambda {var r} -> Int: r)()
    print(total)
