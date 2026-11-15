# expect: 'where' clauses on trait methods are not supported
# A trait default declares no `where` clause, even one a conformer inherits,
# as the pin rejects it.
trait Stepped:
    def step[n: Int, m: Int](self) where n + 1 == m:
        print(n, m)


@fieldwise_init
struct Steps(Stepped):
    pass


def main():
    Steps().step[1, 2]()
