# A place a method call keeps for a `mut` parameter must not alias the
# receiver the same call mutates. The conflict is judged on the two places, so
# the checked template reports it once and no instance is derived around it.
# expect: a mutable borrow must be exclusive
struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var count: Int

    def __init__(out self):
        self.count = 1

    def grow(mut self, mut n: Int):
        self.count += n
        n = self.count

    def regrow(mut self):
        self.grow(self.count)


def main():
    var numbers = Shelf[Int]()
    numbers.regrow()
    print(numbers.count)
