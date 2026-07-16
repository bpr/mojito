def prefer_fixed(x: Int, y: Int) -> Int:
    return 1

def prefer_fixed(*values: Int) -> Int:
    return 2

def prefer_conversion[T: AnyType](value: T) -> Int:
    return 3

def prefer_conversion(value: Float64) -> Int:
    return 4

def main():
    var value: Int = 7
    print(prefer_fixed(1, 2), prefer_fixed(1, 2, 3))
    print(prefer_conversion(value))
