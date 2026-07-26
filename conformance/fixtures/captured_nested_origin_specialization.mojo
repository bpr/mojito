def main():
    var value = 40
    var calls = 0

    def borrow[origin: Origin[mut=True]](
        ref[origin] item: Int
    ) {mut calls} -> ref[origin] Int:
        calls += 1
        return item

    var function = borrow[origin_of(value)]
    ref result = function(value)
    result += 2
    print(value, calls)
