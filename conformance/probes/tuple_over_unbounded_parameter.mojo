# Pin gap probe (Mojo 1.2.0.dev2026092105): a `Tuple[Int, T]` parameter type
# over a type parameter whose bounds do not prove `Movable`. The pin rejects
# the signature ("invalid bindings in signature: lacking evidence to prove
# correctness ... needs evidence for 'conforms_to(T, Movable)'"); Mojito
# accepts it and prints `t` and `12`. Roadmap section 3 carries the entry.
def show[T: Writable](x: T):
    print(x)


def total[T: Writable](pair: Tuple[Int, T]) -> Int:
    show(pair[1])
    return pair[0]


def main():
    print(total((12, String("t"))))
