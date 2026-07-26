def nested() -> Int:
    var base = 40

    def middle() {imm base} -> Int:
        def inner() {imm base} -> Int:
            return base + 2

        return inner()

    return middle()


def main():
    print(nested())
