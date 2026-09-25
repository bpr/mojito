# Does a generic method move its `var` parameter into a field's `append`
# inside `try`/`finally` (and so inside any `with` statement)?
# The pin prints `finally` then `2`. Mojito reports "use of uninitialized
# value 'value'" from the template's own check, even when the method is never
# called; without the `try` it runs. `docs/roadmap.md` 3.25.
struct Bag[T: ImplicitlyCopyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self, var items: List[Self.T]):
        self.items = items^

    def both(mut self, var value: Self.T) -> Int:
        try:
            self.items.append(value^)
            return len(self.items)
        finally:
            print("finally")


def main():
    var strings: List[String] = [String("x")]
    var b = Bag[String](strings^)
    print(b.both(String("y")))
