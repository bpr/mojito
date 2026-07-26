def main():
    var value = 40

    def borrow[origin: Origin[mut=True]](
        ref[origin] item: Int
    ) -> ref[origin] Int:
        return item

    var function = borrow[origin_of(value)]
    ref result = function(value)
    result += 2
    print(value)
