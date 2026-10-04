# expect: aliasing values passed mutably to 'a' argument and passed immutably to 'b' argument in 'both' call
# A homogeneous `*b: Int` element is held by reference too, so `k` passed to
# a `mut` parameter and again into the collector aliases, as at the pin.
def both(mut a: Int, *b: Int) -> Int:
    return a


def main():
    var k = 1
    print(both(k, k))
