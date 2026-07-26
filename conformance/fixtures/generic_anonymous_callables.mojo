def add[n: Int](value: Int) -> Int:
    return value + n


def invoke_value[
    callback: def[n: Int](Int) thin -> Int,
]() -> Int:
    return callback[2](40)


def identity[T: ImplicitlyCopyable & ImplicitlyDeletable](value: T) -> T:
    return value


def invoke_bound[
    F: def[T: ImplicitlyCopyable & ImplicitlyDeletable](T) -> T
](callback: F) -> Int:
    return callback(42)


def invoke_contract_default[
    F: def[n: Int = 2](Int) -> Int
](callback: F) -> Int:
    return callback(40)


def add_with_different_default[width: Int = 3](value: Int) -> Int:
    return value + width


def invoke_captured[
    origins: OriginSet,
    //,
    callback: def[n: Int](Int) capturing[origins] -> Int,
]() -> Int:
    return callback[2](40)


def partial_actual[n: Int = 0, m: Int = 3](value: Int) -> Int:
    return value + n + m


def invoke_partial[
    callback: def[n: Int = 0, m: Int = 2](Int) thin -> Int,
]() -> Int:
    return callback[1](40)


def named_actual[scale: Int = 7, n: Int = 1](value: Int) -> Int:
    return value + scale + n


def invoke_named[
    callback: def[scale: Int = 100, n: Int = 1](Int) thin -> Int,
]() -> Int:
    return callback[n=2](0)


def increment(value: Int) -> Int:
    return value + 1


def decrement(value: Int) -> Int:
    return value - 1


def symbol_actual[
    callback: def(Int) thin -> Int = decrement,
](value: Int) -> Int:
    return callback(value)


def invoke_symbol[
    callback: def[
        operation: def(Int) thin -> Int = increment
    ](Int) thin -> Int,
]() -> Int:
    return callback(41)


def conditional_actual[
    enabled: Bool = False,
    callback: def(Int) thin -> Int = increment if enabled else decrement,
](value: Int) -> Int:
    return callback(value)


def invoke_conditional[
    callback: def[
        enabled: Bool = True,
        operation: def(Int) thin -> Int = increment if enabled else decrement,
    ](Int) thin -> Int,
]() -> Int:
    return callback(41)


def alias_actual[
    primary: def(Int) thin -> Int = decrement,
    fallback: def(Int) thin -> Int = primary,
](value: Int) -> Int:
    return fallback(value)


def invoke_alias[
    callback: def[
        primary: def(Int) thin -> Int = increment,
        fallback: def(Int) thin -> Int = primary,
    ](Int) thin -> Int,
]() -> Int:
    return callback(41)


def main():
    print(invoke_value[add]())
    print(invoke_bound(identity))
    print(invoke_contract_default(add_with_different_default))

    var offset = 0

    @parameter
    def captured_add[n: Int](value: Int) -> Int:
        return value + n + offset

    print(invoke_captured[captured_add]())
    print(invoke_partial[partial_actual]())
    print(invoke_named[named_actual]())
    print(invoke_symbol[symbol_actual]())
    print(invoke_conditional[conditional_actual]())
    print(invoke_alias[alias_actual]())
