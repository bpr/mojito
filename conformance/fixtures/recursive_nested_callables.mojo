def answer() -> Int:
    var total = 40

    def middle() {mut total}:
        def inner() {mut total}:
            def innermost() {mut total}:
                total += 2

            innermost()

        inner()

    middle()
    return total


def main():
    print(answer())
