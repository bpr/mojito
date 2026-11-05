# expect: aliasing values passed mutably to 'a' argument and passed mutably to 'b' argument in 'two' call
# A closure capturing `k` by `mut` holds a mutable reference over `k`'s
# origin: passing it twice to one call aliases two writers of `k`.
def two[F: def() -> None](a: F, b: F):
    a()
    b()


def main():
    var k = 1

    def f() {mut k}:
        k += 1

    two(f, f)
    print(k)
