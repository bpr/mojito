def once[
    origins: OriginSet, //, callback: def() capturing[origins] -> Int
]() -> Int:
    return callback()


def main():
    var value = 42

    @parameter
    def get() -> Int:
        return value

    print(once[get]())
