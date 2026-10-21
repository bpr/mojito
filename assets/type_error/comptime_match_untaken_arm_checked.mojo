# expect: type mismatch for variable 'bad'
# Every case body of a `comptime match` is checked, as every `comptime if`
# arm is, whichever case the elaborator keeps.
def main():
    comptime n = 3
    comptime match n:
        case 1:
            var bad: Int = "str"
            print(bad)
        case _:
            print("other")
