# Every capturing lambda has its own type, its closure's storage, so two of
# them never unify into one display element type, even with identical
# signatures and captures.
# expect: type mismatch for list element
def main():
    var k = 3
    var fns = [lambda (x: Int) {k} -> Int: x * k, lambda (x: Int) {k} -> Int: x + k]
    print(fns[1](2))
