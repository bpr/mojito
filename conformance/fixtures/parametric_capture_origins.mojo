def once[F: def() -> Int](callback: F) -> Int:
    return callback()


def main():
    var value = 42

    def get() {imm value} -> Int:
        return value

    print(once(get))
