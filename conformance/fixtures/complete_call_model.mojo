def sum_values[*ArgTypes: Intable](*args: *ArgTypes) -> Int:
    var total: Int = 0
    comptime for i in range(args.__len__()):
        total = total + Int(args[i])
    return total

struct Box:
    var value: Int

    def __init__(out self, value: Int = 3):
        self.value = value

def main():
    var defaulted = Box()
    var keyword = Box(value=7)
    print(sum_values(1, True, 2.0))
    print(defaulted.value, keyword.value)
