# Materializing an explicit Origin specialization of a nested function that
# has a capture environment is rejected, matching the pinned compiler.
# expect: capture environment
def main():
    var marker = 2
    var value = 40
    def borrow[origin: Origin[mut=True]](ref[origin] item: Int) {imm marker} -> ref[origin] Int:
        print(marker)
        return item
    var function = borrow[origin_of(value)]
    ref result = function(value)
    result += marker
    print(value)
