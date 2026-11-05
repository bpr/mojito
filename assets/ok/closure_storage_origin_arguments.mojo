# A closure's storage struct refers to each reference capture through an
# origin parameter of its own, applied at the closure's value to the
# captured place. A write to the place between the declaration and a call
# is observed, an `imm` capture may be passed twice, and a `mut` capture
# may sit beside a copied argument read from its place.
def use[F: def() -> Int](f: F) -> Int:
    return f()


def two[F: def() -> Int](a: F, b: F) -> Int:
    return a() + b()


def run[F: def() -> Int](f: F, imm y: Int) -> Int:
    return f() + y


def observed():
    var k = 1

    def f() {imm k} -> Int:
        return k

    k = 5
    print(f())


def generic_calls():
    var k = 1

    def f() {imm k} -> Int:
        return k

    k += 3
    print(use(f))
    k += 3
    print(use(f))


def passed_twice():
    var k = 1

    def f() {imm k} -> Int:
        return k

    print(two(f, f))


def beside_copied_argument():
    var k = 1

    def f() {mut k} -> Int:
        k += 1
        return k

    print(run(f, k))


def main():
    observed()
    generic_calls()
    passed_twice()
    beside_copied_argument()
