# expect: aliasing values passed mutably to 'x' argument and passed immutably to 'f' argument in 'g' call
# A closure's storage is applied to the origin of each place it captures by
# reference, so passing it beside a `mut` argument over that place aliases
# them, as at the pin.
def g[F: def() -> Int](mut x: Int, f: F):
    x = 2 + f()


def main():
    var k = 1

    def f() {imm k} -> Int:
        return k

    g(k, f)
    print(k)
