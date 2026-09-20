# expect: invalidated interior reference
# A `ref` local in a generic method derives from the checked template, and the
# facts an instance inherits still say what the binding borrows: the `mut`
# sibling call invalidates the element it names, so the later read is rejected
# for every instance, derived or inferred.
struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var items: List[Self.T]
    var count: Int

    def __init__(out self):
        self.items = List[Self.T]()
        self.count = 0

    def add(mut self, var value: Self.T):
        self.items.append(value^)

    def bump(mut self, by: Int):
        self.count += by

    def grow(mut self, by: Int) -> Self.T:
        ref first = self.items[0]
        self.bump(by)
        return first


def main():
    var numbers = Shelf[Int]()
    numbers.add(4)
    print(numbers.grow(5))
