@fieldwise_init
struct Mutator(def(mut Int) -> None):
    def __call__(self, mut value: Int, /) capturing:
        value += 1


@fieldwise_init
struct Borrower(
    def[origin: Origin[mut=True]](
        ref[origin] Int
    ) -> ref[origin] Int
):
    def __call__[origin: Origin[mut=True]](
        self, ref[origin] value: Int, /
    ) capturing -> ref[origin] Int:
        return value


def main():
    var value = 40
    Mutator()(value)
    ref borrowed = Borrower()(value)
    borrowed += 1
    print(value)
