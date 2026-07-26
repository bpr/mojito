def borrow[origin: Origin[mut=True]](
    ref[origin] value: Int
) -> ref[origin] Int:
    return value


def main():
    var value = 40
    var function = borrow[origin_of(value)]
    ref result = function(value)
    result += 2
    print(value)
