def apply[
    T: Copyable & ImplicitlyDeletable, F: def(T) -> T
](callback: F, value: T) -> T:
    return callback(value)


def increment(value: Int) -> Int:
    return value + 1


def main():
    print(apply(increment, 41))
