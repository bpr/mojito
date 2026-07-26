def increment(value: Int) -> Int:
    return value + 1


def decrement(value: Int) -> Int:
    return value - 1


def identity[T: ImplicitlyCopyable](value: T) -> T:
    return value


def apply[callback: def(Int) thin -> Int = increment](value: Int) -> Int:
    return callback(value)


def dependent[
    callback: def(Int) thin -> Int,
    fallback: def(Int) thin -> Int = callback,
](value: Int) -> Int:
    return fallback(value)


def conditional[
    enabled: Bool = True,
    callback: def(Int) thin -> Int = increment if enabled else decrement,
](value: Int) -> Int:
    return callback(value)


def generic_default[
    callback: def(Int) thin -> Int = identity[Int],
](value: Int) -> Int:
    return callback(value)


def main():
    print(apply(41))
    print(dependent[increment](41))
    print(conditional[True](41))
    print(conditional[callback=decrement](42))
    print(generic_default(42))
